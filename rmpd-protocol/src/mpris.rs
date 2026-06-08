//! MPRIS D-Bus media player interface.
//!
//! Exposes rmpd on the **session** D-Bus as `org.mpris.MediaPlayer2.rmpd`,
//! implementing the `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player`
//! interfaces. This is what makes rmpd discoverable and controllable by desktop
//! environments (GNOME Shell, KDE Plasma), `playerctl`, and multimedia keys —
//! the standard Linux mechanism for "music player detection". (Real MPD relies on
//! the external `mpDris2` bridge for this; rmpd does it natively.)
//!
//! All control methods route through the existing protocol command handlers so
//! that queue advancement, state transitions, and idle events stay consistent
//! with the MPD-protocol surface.

use std::sync::Arc;

use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
    zbus::{Result as ZbusResult, fdo},
};
use rmpd_core::event::Event;
use rmpd_core::song::Song;
use rmpd_core::state::{PlayerState, SingleMode};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use crate::commands::{options, playback};
use crate::state::AppState;

/// Object-path prefix used to mint per-queue-song MPRIS track identifiers.
const TRACK_ID_PREFIX: &str = "/org/rmpd/Track/";

/// Handle that keeps the MPRIS server registered and the event-forwarding task
/// alive. Dropping it releases the D-Bus name and stops forwarding events.
pub struct MprisHandle {
    _server: Arc<Server<MprisPlayer>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MprisHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// MPRIS interface implementation backed by the shared [`AppState`].
pub struct MprisPlayer {
    state: AppState,
}

/// Register the MPRIS interface on the session bus and start forwarding player
/// events as `PropertiesChanged` / `Seeked` signals.
///
/// Returns an error if no session bus is reachable (e.g. a headless host with no
/// D-Bus session); callers should treat that as non-fatal.
pub async fn spawn(state: AppState) -> ZbusResult<MprisHandle> {
    let player = MprisPlayer {
        state: state.clone(),
    };
    let server = Arc::new(Server::new("rmpd", player).await?);
    info!("MPRIS: registered org.mpris.MediaPlayer2.rmpd on the session bus");

    let bus_server = server.clone();
    let task = tokio::spawn(async move {
        let mut rx = state.event_bus.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => forward_event(&bus_server, &state, event).await,
                Err(RecvError::Lagged(n)) => {
                    debug!("MPRIS: event receiver lagged, skipped {n} events");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    Ok(MprisHandle {
        _server: server,
        task,
    })
}

/// Translate a player event into MPRIS property-change / signal emissions.
async fn forward_event(server: &Server<MprisPlayer>, state: &AppState, event: Event) {
    let props: Vec<Property> = match event {
        Event::PlayerStateChanged(s) => {
            let queued = !state.queue.read().await.is_empty();
            vec![
                Property::PlaybackStatus(map_status(s)),
                Property::CanPlay(queued),
                Property::CanPause(true),
                Property::CanGoNext(queued),
                Property::CanGoPrevious(queued),
            ]
        }
        Event::SongChanged(_) => {
            let metadata = build_metadata(state).await;
            let queued = !state.queue.read().await.is_empty();
            vec![
                Property::Metadata(metadata),
                Property::CanGoNext(queued),
                Property::CanGoPrevious(queued),
                Property::CanPlay(queued),
            ]
        }
        Event::VolumeChanged(v) => vec![Property::Volume(f64::from(v) / 100.0)],
        Event::QueueOptionsChanged => {
            let (loop_status, shuffle) = loop_and_shuffle(state).await;
            vec![
                Property::LoopStatus(loop_status),
                Property::Shuffle(shuffle),
            ]
        }
        Event::QueueChanged => {
            let queued = !state.queue.read().await.is_empty();
            vec![
                Property::CanGoNext(queued),
                Property::CanGoPrevious(queued),
                Property::CanPlay(queued),
            ]
        }
        Event::PositionChanged(d) => {
            let position = Time::from_micros(d.as_micros() as i64);
            if let Err(e) = server.emit(Signal::Seeked { position }).await {
                warn!("MPRIS: failed to emit Seeked: {e}");
            }
            return;
        }
        _ => return,
    };

    if let Err(e) = server.properties_changed(props).await {
        warn!("MPRIS: failed to emit PropertiesChanged: {e}");
    }
}

/// Map the daemon's player state to the MPRIS playback status.
fn map_status(state: PlayerState) -> PlaybackStatus {
    match state {
        PlayerState::Play => PlaybackStatus::Playing,
        PlayerState::Pause => PlaybackStatus::Paused,
        PlayerState::Stop => PlaybackStatus::Stopped,
    }
}

/// Read the current player state without taking the engine lock.
fn current_state(state: &AppState) -> PlayerState {
    PlayerState::from_atomic(
        state
            .atomic_state
            .load(std::sync::atomic::Ordering::Acquire),
    )
}

/// Derive the MPRIS loop status and shuffle flag from queue options.
async fn loop_and_shuffle(state: &AppState) -> (LoopStatus, bool) {
    let status = state.status.read().await;
    let loop_status = if !status.repeat {
        LoopStatus::None
    } else if status.single == SingleMode::Off {
        LoopStatus::Playlist
    } else {
        LoopStatus::Track
    };
    (loop_status, status.random)
}

/// Build the MPRIS metadata map for the currently selected queue song.
async fn build_metadata(state: &AppState) -> Metadata {
    let status = state.status.read().await;
    let Some(pos) = status.current_song else {
        return Metadata::new();
    };
    let id = pos.id;
    let length = status.duration;
    drop(status);

    let queue = state.queue.read().await;
    let Some(item) = queue.get_by_id(id) else {
        return Metadata::new();
    };
    let song: Song = (*item.song).clone();
    drop(queue);

    let mut m = Metadata::new();
    m.set_trackid(Some(track_id(id)));
    m.set_title(Some(song.display_title().to_owned()));

    let artists: Vec<String> = song.tag_values("artist").map(str::to_owned).collect();
    if artists.is_empty() {
        m.set_artist(Some([song.display_artist().to_owned()]));
    } else {
        m.set_artist(Some(artists));
    }

    if song.tag("album").is_some() {
        m.set_album(Some(song.display_album().to_owned()));
    }
    if let Some(album_artist) = song.tag_with_fallback("albumartist") {
        m.set_album_artist(Some([album_artist.to_owned()]));
    }
    let genres: Vec<String> = song.tag_values("genre").map(str::to_owned).collect();
    if !genres.is_empty() {
        m.set_genre(Some(genres));
    }
    if let Some(track) = song
        .tag("track")
        .and_then(|t| t.split(['/', ' ']).next())
        .and_then(|t| t.trim().parse::<i32>().ok())
    {
        m.set_track_number(Some(track));
    }
    if let Some(disc) = song.tag("disc").and_then(|d| {
        d.split(['/', ' '])
            .next()
            .and_then(|d| d.trim().parse::<i32>().ok())
    }) {
        m.set_disc_number(Some(disc));
    }
    if let Some(d) = song.duration.or(length) {
        m.set_length(Some(Time::from_micros(d.as_micros() as i64)));
    }
    m.set_url(Some(song_url(&song, state.music_dir.as_deref())));
    m
}

/// Mint a D-Bus object-path track identifier for a queue song id.
fn track_id(id: u32) -> TrackId {
    TrackId::try_from(format!("{TRACK_ID_PREFIX}{id}")).unwrap_or_default()
}

/// Build a `file://` URI (or pass through an existing stream URL) for a song.
fn song_url(song: &Song, music_dir: Option<&str>) -> String {
    let path = song.path.as_str();
    if path.contains("://") {
        return path.to_owned();
    }
    let abs = rmpd_core::path::resolve_path(path, music_dir);
    // Minimal escaping: percent-encode characters that are invalid in a URI path.
    let mut encoded = String::with_capacity(abs.len() + 8);
    for b in abs.bytes() {
        match b {
            b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    format!("file://{encoded}")
}

impl RootInterface for MprisPlayer {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        info!("MPRIS: Quit requested");
        if let Some(tx) = &self.state.shutdown_tx {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> ZbusResult<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("rmpd".to_owned())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("rmpd".to_owned())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec!["file".to_owned()])
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![
            "audio/mpeg".to_owned(),
            "audio/flac".to_owned(),
            "audio/x-flac".to_owned(),
            "audio/ogg".to_owned(),
            "audio/x-vorbis+ogg".to_owned(),
            "audio/mp4".to_owned(),
            "audio/x-wav".to_owned(),
            "audio/aac".to_owned(),
            "audio/x-opus+ogg".to_owned(),
        ])
    }
}

impl PlayerInterface for MprisPlayer {
    async fn next(&self) -> fdo::Result<()> {
        let _ = playback::handle_next_command(&self.state).await;
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        let _ = playback::handle_previous_command(&self.state).await;
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        let _ = playback::handle_pause_command(&self.state, Some(true)).await;
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        match current_state(&self.state) {
            PlayerState::Stop => {
                let _ = playback::handle_play_command(&self.state, None).await;
            }
            _ => {
                let _ = playback::handle_pause_command(&self.state, None).await;
            }
        }
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        let _ = playback::handle_stop_command(&self.state).await;
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        match current_state(&self.state) {
            // Resume from the paused position rather than restarting the track.
            PlayerState::Pause => {
                let _ = playback::handle_pause_command(&self.state, Some(false)).await;
            }
            PlayerState::Stop => {
                let _ = playback::handle_play_command(&self.state, None).await;
            }
            PlayerState::Play => {}
        }
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let secs = offset.as_micros() as f64 / 1_000_000.0;
        let _ = playback::handle_seekcur_command(&self.state, secs, true).await;
        Ok(())
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        // Per spec, ignore the request if the track id is not the current song.
        let current_id = self.state.status.read().await.current_song.map(|p| p.id);
        let requested_id = track_id
            .into_inner()
            .as_str()
            .strip_prefix(TRACK_ID_PREFIX)
            .and_then(|s| s.parse::<u32>().ok());
        if current_id.is_none() || current_id != requested_id {
            return Ok(());
        }
        let secs = (position.as_micros() as f64 / 1_000_000.0).max(0.0);
        let _ = playback::handle_seekcur_command(&self.state, secs, false).await;
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(map_status(current_state(&self.state)))
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(loop_and_shuffle(&self.state).await.0)
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> ZbusResult<()> {
        match loop_status {
            LoopStatus::None => {
                let _ = options::handle_repeat_command(&self.state, false).await;
                let _ = options::handle_single_command(&self.state, "0").await;
            }
            LoopStatus::Playlist => {
                let _ = options::handle_repeat_command(&self.state, true).await;
                let _ = options::handle_single_command(&self.state, "0").await;
            }
            LoopStatus::Track => {
                let _ = options::handle_repeat_command(&self.state, true).await;
                let _ = options::handle_single_command(&self.state, "1").await;
            }
        }
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> ZbusResult<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(loop_and_shuffle(&self.state).await.1)
    }

    async fn set_shuffle(&self, shuffle: bool) -> ZbusResult<()> {
        let _ = options::handle_random_command(&self.state, shuffle).await;
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(build_metadata(&self.state).await)
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        let vol = self.state.status.read().await.volume;
        Ok(f64::from(vol) / 100.0)
    }

    async fn set_volume(&self, volume: Volume) -> ZbusResult<()> {
        let clamped = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        let _ = options::handle_setvol_command(&self.state, clamped).await;
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        let elapsed = self.state.status.read().await.elapsed;
        Ok(elapsed.map_or(Time::ZERO, |d| Time::from_micros(d.as_micros() as i64)))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(!self.state.queue.read().await.is_empty())
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(!self.state.queue.read().await.is_empty())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(!self.state.queue.read().await.is_empty())
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.state.status.read().await.current_song.is_some())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}
