//! Shared utility functions and constants for command handlers

use crate::response::ResponseBuilder;

/// MPD protocol error codes (ACK error types)
/// Reference: <https://mpd.readthedocs.io/en/latest/protocol.html#ack-errors>
pub const ACK_ERROR_ARG: i32 = 2;
pub const ACK_ERROR_PASSWORD: i32 = 3;
pub const ACK_ERROR_PERMISSION: i32 = 4;
pub const ACK_ERROR_UNKNOWN: i32 = 5;
pub const ACK_ERROR_NO_EXIST: i32 = 50;
pub const ACK_ERROR_SYS: i32 = 52;
pub const ACK_ERROR_PLAYER_SYNC: i32 = 55;
pub const ACK_ERROR_EXIST: i32 = 56;

/// Borrow a pooled database connection, returning an error response string on
/// failure. Reuses connections from the shared pool instead of opening a fresh
/// SQLite connection (and re-running schema init) on every command.
pub fn open_db(
    state: &crate::state::AppState,
    command: &str,
) -> Result<rmpd_library::Database, String> {
    let pool = state.db_pool.as_ref().ok_or_else(|| {
        ResponseBuilder::error(ACK_ERROR_SYS, 0, command, "database not configured")
    })?;
    rmpd_library::Database::from_pool(pool).map_err(|e| {
        ResponseBuilder::error(ACK_ERROR_SYS, 0, command, &format!("database error: {e}"))
    })
}

pub use rmpd_core::time::format_iso8601 as format_iso8601_timestamp;

/// Build a FilterExpression from multiple tag/value pairs joined with AND,
/// using `op` as the comparison for every pair (`Equal` for exact matches,
/// `Contains` for case-insensitive substring matching as used by `search`).
/// Panics if `filters` is empty.
pub fn build_filter(
    filters: &[(String, String)],
    op: rmpd_core::filter::CompareOp,
) -> rmpd_core::filter::FilterExpression {
    use rmpd_core::filter::FilterExpression;

    let base = FilterExpression::Compare {
        tag: filters[0].0.clone(),
        op,
        value: filters[0].1.clone(),
    };

    filters[1..].iter().fold(base, |expr, filter| {
        let next_expr = FilterExpression::Compare {
            tag: filter.0.clone(),
            op,
            value: filter.1.clone(),
        };
        FilterExpression::And(Box::new(expr), Box::new(next_expr))
    })
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

/// Insert `song` into the queue at `position`, rejecting an out-of-range
/// position instead of letting `Queue::add_at` silently clamp it to an
/// append. Real MPD replies `Bad song index` when `position > queue length`;
/// `position == queue length` (append) and `None` (append) still succeed.
/// The length check and the insert share one write-lock acquisition so a
/// concurrent `add`/`addid` can't race between "check" and "act".
pub async fn add_at_checked(
    state: &crate::state::AppState,
    song: rmpd_core::song::Song,
    position: Option<u32>,
    command: &str,
) -> Result<u32, String> {
    let mut queue = state.queue.write().await;
    if let Some(pos) = position
        && pos as usize > queue.len()
    {
        return Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "Bad song index",
        ));
    }
    Ok(queue.add_at(song, position))
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

/// Prepare a song for playback by resolving its path.
///
/// When `song.path` is a mount-style virtual path owned by a live music source
/// (e.g. `alarm-music/Artist/Album/<id>.flac`), the path is resolved to a
/// directly-playable `http(s)://` stream URL via the source registry. All other
/// paths (local files or plain `http(s)://` radio streams) are left unchanged.
///
/// Returns a `PlaybackSong` with the resolved path and an optional playback
/// range (CUE virtual tracks / `rangeid`).  Errors when the owning source
/// cannot resolve the URI (unreachable server, unknown id).
pub async fn prepare_song_for_playback(
    song: &rmpd_core::song::Song,
    music_dir: Option<&str>,
    range: Option<(f64, f64)>,
    sources: &std::sync::Arc<rmpd_source::SourceRegistry>,
) -> Result<rmpd_core::playback::PlaybackSong, rmpd_source::SourceError> {
    use std::sync::Arc;
    let path = song.path.as_str();
    // Mount-style source paths (e.g. `alarm-music/Artist/Album/id.flac`) are
    // owned by a live source and resolve to a real `http(s)://` stream URL.
    // Everything else — local relative/absolute paths and plain radio URIs —
    // passes through `resolve_path` unchanged.
    let resolved_path: String = if sources.owns_path(path) {
        // Spawn the resolution onto a Tokio task so the non-Sync async_trait
        // future does not poison the outer future with a non-Sync bound
        // (required by the MPRIS interface).
        let sources = sources.clone();
        let path = path.to_owned();
        tokio::spawn(async move { sources.resolve_stream_uri(&path).await })
            .await
            .map_err(|e| {
                rmpd_source::SourceError::Protocol(format!("resolve task panicked: {e}"))
            })?? // JoinError then SourceError
    } else {
        resolve_path(path, music_dir)
    };
    Ok(rmpd_core::playback::PlaybackSong {
        song: Arc::new(song.clone()),
        resolved_path: resolved_path.into(),
        range,
    })
}

pub use rmpd_core::path::resolve_path;
