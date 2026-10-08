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

use crate::commands::{options, playback};
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
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{debug, info};

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
}

impl MediaSnapshot {
    fn empty() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: None,
            duration: None,
            position: None,
            playing: false,
            stopped: true,
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
    let mtm = MainThreadMarker::new().expect("media controls must start on the main thread");
    let state = ready_rx.recv().expect("server readiness signal");

    {
        let app = NSApplication::sharedApplication(mtm);
        // Background citizen: no Dock icon, no menu bar.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let config = PlatformConfig {
            dbus_name: "rmpd",
            display_name: "rmpd",
            hwnd: None,
        };
        let mut controls = MediaControls::new(config).expect("create media controls");

        // Remote-command handler: route hardware buttons into the same
        // command handlers the MPD protocol uses. The closure must be
        // `Send + 'static`, hence the owned clones; blocking on the runtime
        // handle here is fine because these handlers are quick state flips.
        let event_state = state.clone();
        let event_rt = rt.clone();
        controls
            .attach(move |event| {
                let fut = dispatch_event(&event_state, event);
                if let Err(e) = event_rt.block_on(fut) {
                    debug!("media controls: command failed: {e}");
                }
            })
            .expect("attach remote command handler");

        // Initial snapshot so the Now Playing tile is populated immediately.
        let snap = rt.block_on(snapshot_media_state(&state));
        apply_snapshot(&mut controls, &snap);

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
                        apply_snapshot(controls, &snap);
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
                    let snap = snapshot_media_state(&watch_state).await;
                    if tx.send(PumpMsg::Refresh(snap)).is_err() {
                        break;
                    }
                    // Coalesce bursts — position ticks arrive frequently.
                    while matches!(
                        rx.try_recv(),
                        Ok(Event::PositionChanged(_))
                            | Ok(Event::PlayerStateChanged(_))
                            | Ok(Event::SongChanged(_))
                    ) {}
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
    let position = status.elapsed;
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
        }
    }
    snap
}

/// Push a snapshot into the OS surfaces. Runs on the main thread; the
/// souvlaki metadata borrows from `snap`, which outlives the call.
fn apply_snapshot(controls: &mut MediaControls, snap: &MediaSnapshot) {
    let meta = MediaMetadata {
        title: Some(&snap.title),
        artist: Some(&snap.artist),
        album: snap.album.as_deref(),
        duration: snap.duration,
        cover_url: None,
    };
    controls.set_metadata(meta).ok();

    let progress = snap.position.map(MediaPosition);
    let pb = match (snap.stopped, snap.playing) {
        (true, _) => MediaPlayback::Stopped {},
        (false, true) => MediaPlayback::Playing { progress },
        (false, false) => MediaPlayback::Paused { progress },
    };
    controls.set_playback(pb).ok();
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
        MediaControlEvent::Stop => {
            tracing::debug!("media control:-> stop");
            playback::handle_stop_command(state).await;
        }
        MediaControlEvent::SeekBy(direction, duration) => {
            let sign = match direction {
                souvlaki::SeekDirection::Forward => 1.0,
                souvlaki::SeekDirection::Backward => -1.0,
            };
            playback::handle_seekcur_command(state, sign * duration.as_secs_f64(), true).await;
        }
        MediaControlEvent::SetPosition(pos) => {
            let MediaPosition(target) = pos;
            playback::handle_seekcur_command(state, target.as_secs_f64(), false).await;
        }
        MediaControlEvent::SetVolume(volume) => {
            let vol = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
            options::handle_setvol_command(state, vol).await;
        }
        other => debug!("media controls: ignoring event {other:?}"),
    }
    Ok(())
}

// Silence unused-import warning for platforms where this module compiles but
// the metadata type only flows through souvlaki's own API surface.
#[allow(unused_imports)]
use MediaMetadata as _MediaMetadataUsed;

/// Reference kept so rustc treats `tokio_mpsc` import as used on non-macOS
/// staging builds of this module in IDEs; remove when the module is fully
/// wired behind its cfg gate.
#[allow(dead_code)]
type __TokioMpsc = tokio_mpsc::Sender<()>;
