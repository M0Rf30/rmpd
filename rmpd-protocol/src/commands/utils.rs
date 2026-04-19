//! Shared utility functions and constants for command handlers

use crate::response::ResponseBuilder;

/// MPD protocol error codes (ACK error types)
/// Reference: <https://mpd.readthedocs.io/en/latest/protocol.html#ack-errors>
pub const ACK_ERROR_ARG: i32 = 2;
pub const ACK_ERROR_PASSWORD: i32 = 3;
pub const ACK_ERROR_PERMISSION: i32 = 4;
pub const ACK_ERROR_UNKNOWN: i32 = 5;
pub const ACK_ERROR_NO_EXIST: i32 = 50;
#[allow(dead_code)]
pub const ACK_ERROR_PLAYLIST_MAX: i32 = 51;
#[allow(dead_code)]
pub const ACK_ERROR_SYS: i32 = 52;
#[allow(dead_code)]
pub const ACK_ERROR_PLAYLIST_LOAD: i32 = 53;
#[allow(dead_code)]
pub const ACK_ERROR_UPDATE_ALREADY: i32 = 54;
pub const ACK_ERROR_PLAYER_SYNC: i32 = 55;
pub const ACK_ERROR_EXIST: i32 = 56;
/// Alias kept for backward compat within rmpd (maps to ACK_ERROR_NO_EXIST = 50)
pub const ACK_ERROR_SYSTEM: i32 = ACK_ERROR_NO_EXIST;

/// Open the music database, returning an error response string on failure.
/// This eliminates the repeated db_path check + Database::open boilerplate.
pub fn open_db(
    state: &crate::state::AppState,
    command: &str,
) -> Result<rmpd_library::Database, String> {
    let db_path = state.db_path.as_ref().ok_or_else(|| {
        ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, command, "database not configured")
    })?;
    rmpd_library::Database::open(db_path).map_err(|e| {
        ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            command,
            &format!("database error: {e}"),
        )
    })
}

pub use rmpd_core::time::format_iso8601 as format_iso8601_timestamp;

/// Build a FilterExpression from multiple tag/value pairs joined with AND.
/// Panics if `filters` is empty.
pub fn build_and_filter(filters: &[(String, String)]) -> rmpd_core::filter::FilterExpression {
    use rmpd_core::filter::{CompareOp, FilterExpression};

    let mut expr = FilterExpression::Compare {
        tag: filters[0].0.clone(),
        op: CompareOp::Equal,
        value: filters[0].1.clone(),
    };

    for filter in &filters[1..] {
        let next_expr = FilterExpression::Compare {
            tag: filter.0.clone(),
            op: CompareOp::Equal,
            value: filter.1.clone(),
        };
        expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
    }

    expr
}

/// Build a FilterExpression from multiple tag/value pairs joined with AND,
/// using Contains (LIKE) for case-insensitive substring matching (for `search`).
/// Panics if `filters` is empty.
pub fn build_search_filter(filters: &[(String, String)]) -> rmpd_core::filter::FilterExpression {
    use rmpd_core::filter::{CompareOp, FilterExpression};

    let mut expr = FilterExpression::Compare {
        tag: filters[0].0.clone(),
        op: CompareOp::Contains,
        value: filters[0].1.clone(),
    };

    for filter in &filters[1..] {
        let next_expr = FilterExpression::Compare {
            tag: filter.0.clone(),
            op: CompareOp::Contains,
            value: filter.1.clone(),
        };
        expr = FilterExpression::And(Box::new(expr), Box::new(next_expr));
    }

    expr
}

/// Apply a range/window filter to a slice, returning the filtered sub-slice.
pub fn apply_range<T>(items: &[T], range: Option<(u32, u32)>) -> &[T] {
    if let Some((start, end)) = range {
        let start_idx = start as usize;
        let end_idx = end.min(items.len() as u32) as usize;
        if start_idx < items.len() {
            &items[start_idx..end_idx]
        } else {
            &[]
        }
    } else {
        items
    }
}

/// Append queue item metadata (priority, range) to the response.
pub fn add_queue_item_metadata(
    resp: &mut crate::response::ResponseBuilder,
    item: &rmpd_core::queue::QueueItem,
) {
    if item.priority > 0 {
        resp.field("Prio", item.priority);
    }
    if let Some((start, end)) = item.range {
        resp.field("Range", format!("{start:.3}-{end:.3}"));
    }
}

/// Update next_song in status based on the current position in the queue.
pub fn update_next_song(
    status: &mut rmpd_core::state::PlayerStatus,
    queue: &rmpd_core::queue::Queue,
    current_pos: u32,
) {
    status.next_song =
        queue
            .get(current_pos + 1)
            .map(|next_item| rmpd_core::state::QueuePosition {
                position: current_pos + 1,
                id: next_item.id,
            });
}

/// Clone a song and resolve its path to an absolute path for playback.
pub fn prepare_song_for_playback(
    song: &rmpd_core::song::Song,
    music_dir: Option<&str>,
) -> rmpd_core::song::Song {
    let mut playback_song = song.clone();
    playback_song.path = resolve_path(song.path.as_str(), music_dir).into();
    playback_song
}

pub use rmpd_core::path::resolve_path;
