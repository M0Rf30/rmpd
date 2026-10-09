// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! MPRIS D-Bus media player integration.
//!
//! Exposes rmpd on the **session** D-Bus as `org.mpris.MediaPlayer2.rmpd`,
//! implementing the `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player`
//! interfaces. This is what makes rmpd discoverable and controllable by desktop
//! environments (GNOME Shell, KDE Plasma), `playerctl`, and multimedia keys.
//! (Real MPD relies on the external `mpDris2` bridge; rmpd does it natively.)
//!
//! All control goes through the [`PlayerHandle`], which routes into the same
//! command handlers the MPD protocol uses, so queue advancement, state
//! transitions and idle events stay consistent.
//!
//! Enabled implicitly by `network.media_controls` (alias `mpris`), or
//! explicitly with `[[integration]] type = "mpris"`.

use async_trait::async_trait;
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
    zbus::{Result as ZbusResult, fdo},
};
use rmpd_core::config::IntegrationConfig;
use rmpd_core::event::Event;
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{
    Integration, IntegrationContext, IntegrationPlugin, PlayerHandle, PlayerOptions,
};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

/// Settings accepted by the MPRIS integration (none).
pub const SETTINGS: &[&str] = &[];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "mpris",
    settings: SETTINGS,
    factory,
};

/// Object-path prefix used to mint per-queue-song MPRIS track identifiers.
const TRACK_ID_PREFIX: &str = "/org/rmpd/Track/";

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    Ok(Box::new(MprisIntegration {
        name: cfg.name.clone(),
    }))
}

/// The MPRIS integration.
pub struct MprisIntegration {
    name: String,
}

#[async_trait]
impl Integration for MprisIntegration {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let IntegrationContext {
            mut events,
            player,
            mut shutdown,
            ..
        } = ctx;

        let server = Server::new(
            "rmpd",
            MprisPlayer {
                player: Arc::clone(&player),
            },
        )
        .await
        .map_err(|e| PluginError::Unavailable(format!("MPRIS interface disabled: {e}")))?;
        info!("MPRIS: registered org.mpris.MediaPlayer2.rmpd on the session bus");

        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                ev = events.recv() => match ev {
                    Ok(event) => forward_event(&server, &player, event).await,
                    Err(RecvError::Lagged(n)) => {
                        debug!("MPRIS: event receiver lagged, skipped {n} events");
                    }
                    Err(RecvError::Closed) => break,
                },
            }
        }
        // Dropping `server` releases the D-Bus name.
        Ok(())
    }
}

/// MPRIS interface implementation backed by a [`PlayerHandle`].
pub struct MprisPlayer {
    player: Arc<dyn PlayerHandle>,
}

/// Translate a player event into MPRIS property-change / signal emissions.
async fn forward_event(server: &Server<MprisPlayer>, player: &Arc<dyn PlayerHandle>, event: Event) {
    let props: Vec<Property> = match event {
        Event::PlayerStateChanged(s) => {
            let queued = player.queue_len().await > 0;
            vec![
                Property::PlaybackStatus(map_status(s)),
                Property::CanPlay(queued),
                Property::CanPause(true),
                Property::CanGoNext(queued),
                Property::CanGoPrevious(queued),
            ]
        }
        Event::SongChanged(_) => {
            let metadata = build_metadata(player.as_ref()).await;
            let queued = player.queue_len().await > 0;
            vec![
                Property::Metadata(metadata),
                Property::CanGoNext(queued),
                Property::CanGoPrevious(queued),
                Property::CanPlay(queued),
            ]
        }
        Event::VolumeChanged(v) => vec![Property::Volume(f64::from(v) / 100.0)],
        Event::QueueOptionsChanged => {
            let opts = player.options().await;
            vec![
                Property::LoopStatus(loop_status(opts)),
                Property::Shuffle(opts.random),
            ]
        }
        Event::QueueChanged => {
            let queued = player.queue_len().await > 0;
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

/// Derive the MPRIS loop status from queue options.
fn loop_status(opts: PlayerOptions) -> LoopStatus {
    if !opts.repeat {
        LoopStatus::None
    } else if !opts.single {
        LoopStatus::Playlist
    } else {
        LoopStatus::Track
    }
}

/// Build the MPRIS metadata map for the currently selected queue song.
async fn build_metadata(player: &dyn PlayerHandle) -> Metadata {
    let Some(id) = player.current_song_id().await else {
        return Metadata::new();
    };
    let snapshot = player.status().await;
    let Some(song) = snapshot.song else {
        return Metadata::new();
    };
    metadata_for(&song, id, snapshot.duration, player.music_dir().as_deref())
}

/// Pure metadata mapping for one song.
fn metadata_for(
    song: &Song,
    id: u32,
    length: Option<std::time::Duration>,
    music_dir: Option<&str>,
) -> Metadata {
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
    if let Some(track) = song.tag("track").and_then(parse_leading_number) {
        m.set_track_number(Some(track));
    }
    if let Some(disc) = song.tag("disc").and_then(parse_leading_number) {
        m.set_disc_number(Some(disc));
    }
    if let Some(d) = song.duration.or(length) {
        m.set_length(Some(Time::from_micros(d.as_micros() as i64)));
    }
    m.set_url(Some(song_url(song, music_dir)));
    m
}

/// Parse the number before a `/` or space (`"3/12"` -> 3).
fn parse_leading_number(s: &str) -> Option<i32> {
    s.split(['/', ' '])
        .next()
        .and_then(|t| t.trim().parse::<i32>().ok())
}

/// Mint a D-Bus object-path track identifier for a queue song id.
fn track_id(id: u32) -> TrackId {
    TrackId::try_from(format!("{TRACK_ID_PREFIX}{id}")).unwrap_or_default()
}

/// Build a `file://` URI (or pass through an existing stream URL) for a song.
fn song_url(song: &Song, music_dir: Option<&str>) -> String {
    path_url(song.path.as_str(), music_dir)
}

/// Path/URL -> URI (see [`song_url`]).
fn path_url(path: &str, music_dir: Option<&str>) -> String {
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
        self.player.request_shutdown();
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

impl MprisPlayer {
    /// Run a `PlayerHandle` call on the tokio runtime. `mpris-server` needs
    /// `Send + Sync` futures, but `async_trait` futures are only `Send`;
    /// awaiting a `JoinHandle` is both.
    fn on<T, F, Fut>(
        &self,
        f: F,
    ) -> impl std::future::Future<Output = T> + Send + Sync + use<T, F, Fut>
    where
        T: Send + 'static,
        F: FnOnce(Arc<dyn PlayerHandle>) -> Fut,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        let handle = tokio::spawn(f(Arc::clone(&self.player)));
        async move {
            match handle.await {
                Ok(v) => v,
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            }
        }
    }
}

impl PlayerInterface for MprisPlayer {
    async fn next(&self) -> fdo::Result<()> {
        let _ = self.on(move |p| async move { p.next().await }).await;
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        let _ = self.on(move |p| async move { p.previous().await }).await;
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        let _ = self.on(move |p| async move { p.pause().await }).await;
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        let _ = self.on(move |p| async move { p.toggle().await }).await;
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        let _ = self.on(move |p| async move { p.stop().await }).await;
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        match self
            .on(move |p| async move { p.status().await })
            .await
            .state
        {
            // Resume from the paused position rather than restarting the track
            // (`toggle` on a paused player resumes).
            PlayerState::Pause => {
                let _ = self.on(move |p| async move { p.toggle().await }).await;
            }
            PlayerState::Stop => {
                let _ = self.on(move |p| async move { p.play().await }).await;
            }
            PlayerState::Play => {}
        }
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let secs = offset.as_micros() as f64 / 1_000_000.0;
        let _ = self
            .on(move |p| async move { p.seek_relative(secs).await })
            .await;
        Ok(())
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        // Per spec, ignore the request if the track id is not the current song.
        let current_id = self
            .on(move |p| async move { p.current_song_id().await })
            .await;
        let requested_id = track_id
            .into_inner()
            .as_str()
            .strip_prefix(TRACK_ID_PREFIX)
            .and_then(|s| s.parse::<u32>().ok());
        if current_id.is_none() || current_id != requested_id {
            return Ok(());
        }
        let micros = position.as_micros().max(0) as u64;
        let _ = self
            .on(move |p| async move { p.seek(std::time::Duration::from_micros(micros)).await })
            .await;
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(map_status(
            self.on(move |p| async move { p.status().await })
                .await
                .state,
        ))
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(loop_status(
            self.on(move |p| async move { p.options().await }).await,
        ))
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> ZbusResult<()> {
        let (repeat, single) = match loop_status {
            LoopStatus::None => (false, false),
            LoopStatus::Playlist => (true, false),
            LoopStatus::Track => (true, true),
        };
        let _ = self
            .on(move |p| async move { p.set_repeat(repeat).await })
            .await;
        let _ = self
            .on(move |p| async move { p.set_single(single).await })
            .await;
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> ZbusResult<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self
            .on(move |p| async move { p.options().await })
            .await
            .random)
    }

    async fn set_shuffle(&self, shuffle: bool) -> ZbusResult<()> {
        let _ = self
            .on(move |p| async move { p.set_random(shuffle).await })
            .await;
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self
            .on(|p| async move { build_metadata(p.as_ref()).await })
            .await)
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(f64::from(
            self.on(move |p| async move { p.status().await })
                .await
                .volume,
        ) / 100.0)
    }

    async fn set_volume(&self, volume: Volume) -> ZbusResult<()> {
        let clamped = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        let _ = self
            .on(move |p| async move { p.set_volume(clamped).await })
            .await;
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        let elapsed = self.on(move |p| async move { p.position().await }).await;
        Ok(elapsed.map_or(Time::ZERO, |d| Time::from_micros(d.as_micros() as i64)))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.on(move |p| async move { p.queue_len().await }).await > 0)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.on(move |p| async move { p.queue_len().await }).await > 0)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.on(move |p| async move { p.queue_len().await }).await > 0)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        // A stopped player keeps its current song but cannot seek in it.
        let snap = self.on(move |p| async move { p.status().await }).await;
        Ok(snap.state != PlayerState::Stop && snap.song.is_some())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_status_mapping() {
        let o = |repeat, single| PlayerOptions {
            repeat,
            random: false,
            single,
        };
        assert_eq!(loop_status(o(false, false)), LoopStatus::None);
        assert_eq!(loop_status(o(false, true)), LoopStatus::None);
        assert_eq!(loop_status(o(true, false)), LoopStatus::Playlist);
        assert_eq!(loop_status(o(true, true)), LoopStatus::Track);
    }

    #[test]
    fn leading_number_parsing() {
        assert_eq!(parse_leading_number("3/12"), Some(3));
        assert_eq!(parse_leading_number("7"), Some(7));
        assert_eq!(parse_leading_number("x"), None);
    }

    #[test]
    fn urls_pass_through_and_files_are_escaped() {
        assert_eq!(
            path_url("http://example.com/a.mp3", None),
            "http://example.com/a.mp3"
        );
        assert_eq!(
            path_url("/music/a b.flac", None),
            "file:///music/a%20b.flac"
        );
    }
}
