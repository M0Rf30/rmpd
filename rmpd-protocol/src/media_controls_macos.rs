// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Native macOS media integration: Control Center / lock-screen Now Playing
//! entry plus hardware remote commands (headphone play/pause/next/previous).
//!
//! Mirrors the role `mpris.rs` plays on Linux desktops: it observes the same
//! `EventBus` for state/metadata changes and routes user intent into the very
//! same `commands::*` handlers the MPD protocol uses — one command path for
//! every front end.
//!
//! Threading contract: AppKit demands that `NSApplication` lives on the
//! process main thread (`MainThreadMarker` enforces this at compile time).
//! The tokio runtime therefore runs on a background thread (see
//! `rmpd/src/main.rs`) while this module owns the main thread's run loop; the
//! two halves exchange data through plain channels. An `NSTimer` on the main
//! run loop drains those channels because souvlaki's controls must be touched
//! from the main thread only.

use crate::commands::playback;
use crate::state::AppState;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::NSTimer;
use rmpd_core::event::Event;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use tracing::{debug, info, warn};

/// Owned playback picture handed from the async half to the main thread;
/// souvlaki's metadata type borrows `&str`s, so the strings must outlive the
/// `set_metadata` call — hence this plain-data carrier instead of passing the
/// borrowed struct across threads.
#[derive(Clone)]
struct MediaSnapshot {
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<std::time::Duration>,
    position: Option<std::time::Duration>,
    playing: bool,
    stopped: bool,
    /// `file://` URL of the track's picture, when it has one.
    cover_url: Option<String>,
}

impl MediaSnapshot {
    /// Identity of the metadata block. Position is deliberately excluded: it
    /// changes every tick, and pushing metadata again makes the OS reload the
    /// artwork (visible as the panel flickering between the art and nothing).
    fn meta_key(&self) -> String {
        let mut key = String::with_capacity(64);
        for part in [
            self.title.as_str(),
            self.artist.as_str(),
            self.album.as_deref().unwrap_or(""),
            self.cover_url.as_deref().unwrap_or(""),
        ] {
            key.push_str(part);
            key.push('\u{1}');
        }
        key.push_str(&format!("{:?}", self.duration));
        key
    }

    fn empty() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: None,
            duration: None,
            position: None,
            playing: false,
            stopped: true,
            cover_url: None,
        }
    }
}

/// Messages from the async half (event-bus watcher / shutdown watcher) to the
/// main-thread run loop.
enum PumpMsg {
    /// Apply this snapshot to the OS surfaces.
    Refresh(MediaSnapshot),
    /// The server finished; terminate the application loop so the process can
    /// exit cleanly.
    Exit,
}

/// Own the process main thread until the daemon exits.
///
/// `ready_rx` yields the `AppState` once the server is about to start
/// listening; `rt` is the handle to the runtime running on a background
/// thread; `exit_tx` receives a message when the async server half finished
/// so the loop below can terminate the application cleanly.
pub fn run_blocking(
    ready_rx: Receiver<AppState>,
    exit_rx: Receiver<()>,
    rt: tokio::runtime::Handle,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        warn!("media controls unavailable: not on the main thread");
        wait_for_server(&exit_rx);
        return;
    };
    let Ok(state) = ready_rx.recv() else {
        warn!("media controls unavailable: the server stopped before the UI started");
        return;
    };

    {
        let app = NSApplication::sharedApplication(mtm);
        // Background citizen: no Dock icon, no menu bar.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let config = PlatformConfig {
            dbus_name: "rmpd",
            display_name: "rmpd",
            hwnd: None,
        };
        let mut controls = match MediaControls::new(config) {
            Ok(controls) => controls,
            Err(e) => {
                warn!("media controls unavailable: {e}");
                wait_for_server(&exit_rx);
                return;
            }
        };

        // Remote-command handler: route hardware buttons into the same
        // command handlers the MPD protocol uses. The closure must be
        // `Send + 'static`, hence the owned clones; blocking on the runtime
        // handle here is fine because these handlers are quick state flips.
        let event_state = state.clone();
        let event_rt = rt.clone();
        if let Err(e) = controls.attach(move |event| {
            let fut = dispatch_event(&event_state, event);
            if let Err(e) = event_rt.block_on(fut) {
                debug!("media controls: command failed: {e}");
            }
        }) {
            warn!("media controls unavailable: {e}");
            wait_for_server(&exit_rx);
            return;
        }

        // Initial snapshot so the Now Playing tile is populated immediately.
        let snap = rt.block_on(snapshot_media_state(&state));
        let published = std::cell::RefCell::new(None::<String>);
        apply_snapshot(&mut controls, &snap, &mut published.borrow_mut());

        // Async half: forward bus events as refresh requests, watch shutdown.
        let (tx, rx) = std::sync::mpsc::channel::<PumpMsg>();
        spawn_watchers(&rt, &state, tx, Some(exit_rx));

        info!("macOS media controls active (Now Playing + remote commands)");

        // Main-thread pump: an NSTimer on this run loop drains the channel
        // and applies updates to the controls.
        let cell: Rc<std::cell::RefCell<Option<MediaControls>>> =
            Rc::new(std::cell::RefCell::new(Some(controls)));
        let timer_cell = Rc::clone(&cell);
        let block = block2::RcBlock::new(move |_timer: NonNull<NSTimer>| {
            let mut guard = timer_cell.borrow_mut();
            let Some(controls) = guard.as_mut() else {
                return;
            };
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    PumpMsg::Refresh(snap) => {
                        apply_snapshot(controls, &snap, &mut published.borrow_mut());
                    }
                    PumpMsg::Exit => {
                        controls.set_playback(MediaPlayback::Stopped {}).ok();
                        drop(guard);
                        let mtm2 = MainThreadMarker::new().expect("main");
                        NSApplication::sharedApplication(mtm2).terminate(None);
                        return;
                    }
                }
            }
        });
        // SAFETY: creating+scheduling a block-based NSTimer; documented safe
        // on the main thread, which this code owns by contract.
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.4, true, &block) };

        app.run();
        // Keep `cell` alive until after the run loop ends.
        drop(cell);
    }
}

/// Block the main thread until the async half finishes, so a daemon without
/// media controls still exits when the server does.
fn wait_for_server(exit_rx: &Receiver<()>) {
    let _ = exit_rx.recv();
}

/// Spawn the async watchers: one translating bus events into refresh pings,
/// one translating server completion into `Exit`.
fn spawn_watchers(
    rt: &tokio::runtime::Handle,
    state: &AppState,
    tx: Sender<PumpMsg>,
    exit_rx: Option<Receiver<()>>,
) {
    let watch_state = state.clone();
    let refresh_tx = tx.clone();
    rt.spawn(async move {
        let tx = refresh_tx;
        let mut rx = watch_state.event_bus.subscribe();
        loop {
            match rx.recv().await {
                Ok(Event::SongChanged(_))
                | Ok(Event::PlayerStateChanged(_))
                | Ok(Event::PositionChanged(_)) => {
                    // Coalesce bursts first: position ticks arrive frequently.
                    // Draining after the snapshot would drop an event that
                    // arrived while it was taken, leaving the panel stale.
                    while matches!(
                        rx.try_recv(),
                        Ok(Event::PositionChanged(_))
                            | Ok(Event::PlayerStateChanged(_))
                            | Ok(Event::SongChanged(_))
                    ) {}

                    let snap = snapshot_media_state(&watch_state).await;
                    if tx.send(PumpMsg::Refresh(snap)).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("media controls: lagged {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    if let Some(exit_rx) = exit_rx {
        rt.spawn(async move {
            // The async server half sends once it finishes (shutdown or fatal
            // error); relay that into the main-thread pump channel.
            let _ = tokio::task::spawn_blocking(move || {
                let _ = exit_rx.recv();
                tx.send(PumpMsg::Exit)
            })
            .await;
        });
    } else {
        let _ = tx.send(PumpMsg::Exit);
    }
}

async fn snapshot_media_state(state: &AppState) -> MediaSnapshot {
    use rmpd_core::state::PlayerState;

    let status = state.status.read().await;
    // `status.elapsed` is refreshed about once a second by another subscriber,
    // so the panel would lag. Ask the engine for the audible position instead.
    let position = state.engine.read().await.get_elapsed_live();
    let playing = matches!(status.state, PlayerState::Play);
    let stopped = matches!(status.state, PlayerState::Stop);
    let song_id = status.current_song.map(|p| p.id);
    drop(status);

    let mut snap = MediaSnapshot::empty();
    snap.position = position;
    snap.playing = playing;
    snap.stopped = stopped;

    if let Some(id) = song_id {
        let item = {
            let queue = state.queue.read().await;
            queue.get_by_id(id).map(|i| (*i.song).clone())
        };
        if let Some(song) = item {
            snap.title = song.display_title().to_owned();
            snap.artist = song.display_artist().to_owned();
            snap.album = song.tag("album").map(str::to_owned);
            snap.duration = song.duration;
            snap.cover_url = crate::now_playing_art::artwork_url_for_song(
                state.music_dir.as_deref(),
                song.path.as_str(),
            );
        }
    }
    snap
}

/// Push a snapshot into the OS surfaces. Runs on the main thread; the
/// souvlaki metadata borrows from `snap`, which outlives the call.
fn apply_snapshot(
    controls: &mut MediaControls,
    snap: &MediaSnapshot,
    published: &mut Option<String>,
) {
    let key = snap.meta_key();
    if snap.title.is_empty() {
        // Nothing is loaded: leave the panel instead of publishing an empty
        // title, which macOS replaces with the process name.
        if published.take().is_some() {
            clear_now_playing();
        }
    } else if published.as_deref() != Some(key.as_str()) {
        let meta = MediaMetadata {
            title: Some(&snap.title),
            artist: Some(&snap.artist),
            album: snap.album.as_deref(),
            duration: snap.duration,
            cover_url: snap.cover_url.as_deref(),
        };
        controls.set_metadata(meta).ok();
        *published = Some(key);
    }

    let progress = snap.position.map(MediaPosition);
    let pb = match (snap.stopped, snap.playing) {
        (true, _) => MediaPlayback::Stopped {},
        (false, true) => MediaPlayback::Playing { progress },
        (false, false) => MediaPlayback::Paused { progress },
    };
    controls.set_playback(pb).ok();
}

/// Drop the Now Playing entry. souvlaki always writes a metadata dictionary, so
/// an empty one leaves macOS showing the process name; clearing the dictionary is
/// what removes the entry, and souvlaki does not expose it.
fn clear_now_playing() {
    // SAFETY: runs on the main thread, and the selector is valid whenever
    // MediaPlayer is loaded, which it is once souvlaki has attached.
    unsafe {
        let Some(center_class) = objc2::runtime::AnyClass::get(c"MPNowPlayingInfoCenter") else {
            return;
        };
        let center: *mut objc2::runtime::AnyObject = objc2::msg_send![center_class, defaultCenter];
        let nil: Option<&objc2::runtime::AnyObject> = None;
        let _: () = objc2::msg_send![center, setNowPlayingInfo: nil];
    }
}

async fn dispatch_event(state: &AppState, event: MediaControlEvent) -> Result<(), String> {
    tracing::debug!(event = ?event, "media control: received");
    match event {
        MediaControlEvent::Play => {
            tracing::debug!("media control:-> play");
            // Unpause rather than (re)start: MPD clients unpause with
            // `pause 0`, and a bare `play` reopens the track from zero,
            // which trips the gapless bookkeeping into an instant EOF.
            let paused = {
                let status = state.status.read().await;
                matches!(status.state, rmpd_core::state::PlayerState::Pause)
            };
            if paused {
                playback::handle_pause_command(state, Some(false)).await;
            } else {
                playback::handle_play_command(state, None).await;
            }
        }
        MediaControlEvent::Pause => {
            tracing::debug!("media control:-> pause");
            playback::handle_pause_command(state, Some(true)).await;
        }
        MediaControlEvent::Toggle => {
            tracing::debug!("media control:-> toggle-pause");
            playback::handle_pause_command(state, None).await;
        }
        MediaControlEvent::Next => {
            tracing::debug!("media control:-> next");
            playback::handle_next_command(state).await;
        }
        MediaControlEvent::Previous => {
            tracing::debug!("media control:-> previous");
            playback::handle_previous_command(state).await;
        }
        MediaControlEvent::SetPosition(pos) => {
            let MediaPosition(target) = pos;
            playback::handle_seekcur_command(state, target.as_secs_f64(), false).await;
        }
        other => debug!("media controls: ignoring event {other:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn snapshot(title: &str, cover: Option<&str>, position_ms: u64) -> MediaSnapshot {
        MediaSnapshot {
            title: title.to_owned(),
            artist: "Artist".to_owned(),
            album: Some("Album".to_owned()),
            duration: Some(Duration::from_secs(180)),
            position: Some(Duration::from_millis(position_ms)),
            playing: true,
            stopped: false,
            cover_url: cover.map(str::to_owned),
        }
    }

    #[test]
    fn the_metadata_key_ignores_the_position() {
        let first = snapshot("Title", Some("file:///a.png"), 0).meta_key();
        let later = snapshot("Title", Some("file:///a.png"), 42_000).meta_key();
        assert_eq!(first, later, "a position tick must not republish metadata");
    }

    #[test]
    fn the_metadata_key_follows_the_track_and_the_cover() {
        let base = snapshot("Title", Some("file:///a.png"), 0).meta_key();
        assert_ne!(base, snapshot("Other", Some("file:///a.png"), 0).meta_key());
        assert_ne!(base, snapshot("Title", Some("file:///b.png"), 0).meta_key());
        assert_ne!(base, snapshot("Title", None, 0).meta_key());
    }
}
