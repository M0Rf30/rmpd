//! Shared `pub(crate)` helpers for protocol command handlers.

use crate::commands::utils::{
    ACK_ERROR_ARG, ACK_ERROR_SYSTEM, build_and_filter, build_search_filter,
};
use crate::response::ResponseBuilder;
use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::song::{AudioFormat, Song};
use rmpd_core::state::PlayerState;

/// Acquires a write lock on `state.status` and a read lock on `state.queue`.
pub(crate) async fn update_playlist_version(state: &AppState) {
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;
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

/// `case_sensitive=true` → exact match (`find`), `false` → substring/FTS (`search`).
pub(crate) fn resolve_filters(
    db: &rmpd_library::Database,
    filters: &[(String, String)],
    command: &str,
    case_sensitive: bool,
) -> Result<Vec<Song>, String> {
    if filters.is_empty() {
        return Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "missing arguments",
        ));
    }

    if filters[0].0.starts_with('(') {
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => db.find_songs_filter(&filter).map_err(|e| {
                ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, &format!("query error: {e}"))
            }),
            Err(e) => Err(ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                command,
                &format!("filter parse error: {e}"),
            )),
        }
    } else if filters.len() == 1 {
        if case_sensitive {
            db.find_songs(&filters[0].0, &filters[0].1).map_err(|e| {
                ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, &format!("query error: {e}"))
            })
        } else {
            let tag = &filters[0].0;
            let value = &filters[0].1;
            if tag.eq_ignore_ascii_case("any") {
                db.search_songs(value).map_err(|e| {
                    ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        command,
                        &format!("search error: {e}"),
                    )
                })
            } else {
                db.search_songs_by_tag(tag, value).map_err(|e| {
                    ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        command,
                        &format!("query error: {e}"),
                    )
                })
            }
        }
    } else {
        let expr = if case_sensitive {
            build_and_filter(filters)
        } else {
            build_search_filter(filters)
        };
        db.find_songs_filter(&expr).map_err(|e| {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, &format!("query error: {e}"))
        })
    }
}
