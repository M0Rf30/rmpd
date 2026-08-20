//! Shared `pub(crate)` helpers for protocol command handlers.

use crate::commands::utils::{ACK_ERROR_SYS, filter_parse_ack, parse_filter_args};
use crate::response::ResponseBuilder;
use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::song::{AudioFormat, Song};
use rmpd_core::state::PlayerState;

/// Bump the queue (playlist) version and length, then notify the `playlist`
/// idle subsystem so event-driven clients (rmpc, ncmpcpp, …) refetch the queue
/// after it changes. Acquires a write lock on `state.status` and a read lock on
/// `state.queue`.
pub(crate) async fn update_playlist_version(state: &AppState) {
    {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        status.playlist_length = state.queue.read().await.len() as u32;
    }
    state.event_bus.emit(Event::QueueChanged);
}

pub(crate) fn is_known_uri_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "http"
            | "https"
            | "ftp"
            | "ftps"
            | "rtsp"
            | "rtsps"
            | "rtmp"
            | "rtmpe"
            | "rtmps"
            | "rtmpt"
            | "rtmpte"
            | "rtmpts"
            | "rtp"
            | "mms"
            | "mmsh"
            | "mmst"
            | "mmsu"
            | "hls+http"
            | "hls+https"
            | "nfs"
            | "smb"
            | "scp"
            | "sftp"
            | "srtp"
            | "gopher"
            | "alsa"
            | "cdda"
            | "file"
    )
}

pub(crate) fn create_stream_song(uri: &str) -> Song {
    Song {
        id: 0,
        path: camino::Utf8PathBuf::from(uri),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![],
    }
}

/// Sets `status.state` and emits `PlayerStateChanged`. Call-sites needing
/// additional status mutations (e.g. clearing `current_song`) do so separately.
pub(crate) async fn update_player_state(state: &AppState, new_state: PlayerState) {
    state.status.write().await.state = new_state;
    state.event_bus.emit(Event::PlayerStateChanged(new_state));
}

pub(crate) fn extract_audio_format(song: &Song) -> Option<AudioFormat> {
    match (song.sample_rate, song.channels, song.bits_per_sample) {
        (Some(sr), Some(ch), Some(bps)) => Some(AudioFormat {
            sample_rate: sr,
            channels: ch,
            bits_per_sample: bps as u8,
        }),
        _ => None,
    }
}

/// `case_sensitive=true` → `find`-family semantics (case-sensitive equality
/// by default), `false` → `search`-family (case-insensitive substring by
/// default). Accepts both the modern `(...)` expression syntax and the
/// legacy `TAG VALUE [TAG VALUE ...]` pair syntax; see
/// [`rmpd_core::filter::FilterExpression`] for the shared grammar.
pub(crate) fn resolve_filters(
    db: &rmpd_library::Database,
    filters: &[(String, String)],
    command: &str,
    case_sensitive: bool,
) -> Result<Vec<Song>, String> {
    let expr =
        parse_filter_args(filters, !case_sensitive).map_err(|e| filter_parse_ack(command, &e))?;
    db.find_songs_filter(&expr).map_err(|e| {
        ResponseBuilder::error(ACK_ERROR_SYS, 0, command, &format!("query error: {e}"))
    })
}
