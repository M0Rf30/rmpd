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

/// Build a `FilterExpression` from a command's parsed argument list,
/// detecting whether it's a single modern `(...)` expression token (per
/// `parser::parse_find_search_filters`'s convention: `[(expr, "")]`) or
/// legacy `TAG VALUE [TAG VALUE ...]` pairs — mirroring the dispatch
/// `SongFilter::Parse` itself does on `args.front()[0] == '('`.
///
/// `fold_case`: `true` for `search`-family commands (case-insensitive),
/// `false` for `find`-family commands (case-sensitive).
pub fn parse_filter_args(
    filters: &[(String, String)],
    fold_case: bool,
) -> rmpd_core::error::Result<rmpd_core::filter::FilterExpression> {
    if filters.len() == 1 && filters[0].0.starts_with('(') {
        rmpd_core::filter::FilterExpression::parse(&filters[0].0, fold_case)
    } else {
        rmpd_core::filter::FilterExpression::from_pairs(filters, fold_case)
    }
}

/// Convert a filter-expression parse error into an `ACK_ERROR_ARG` response,
/// using the raw MPD-style message text (stripping the `RmpdError` Display
/// wrapper, same convention used for `RmpdError::Library` elsewhere).
pub fn filter_parse_ack(command: &str, err: &rmpd_core::error::RmpdError) -> String {
    let msg = err.to_string();
    let msg = msg.strip_prefix("Parse error: ").unwrap_or(&msg);
    ResponseBuilder::error(ACK_ERROR_ARG, 0, command, msg)
}

/// A resolved `sort TAG` clause: either one of MPD's two synthetic sort
/// keys (`Last-Modified`, `Added`) or a real tag name (already validated
/// and lowercased).
pub enum SortKey {
    LastModified,
    Added,
    Tag(String),
}

/// Parse a `sort TAG` argument, mirroring `ParseSortTag()`: a leading `-`
/// requests descending order; `Last-Modified`/`Added` are synthetic sort
/// keys, anything else must be a known tag name. Returns `None` for an
/// unknown tag — the caller reports MPD's exact `"Unknown sort tag"` text.
pub fn parse_sort_tag(s: &str) -> Option<(SortKey, bool)> {
    let (descending, rest) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let key = if rest.eq_ignore_ascii_case("last-modified") {
        SortKey::LastModified
    } else if rest.eq_ignore_ascii_case("added") {
        SortKey::Added
    } else {
        let tag_lower = rest.to_lowercase();
        if rmpd_core::song::canonical_tag_name(&tag_lower) == "Unknown" {
            return None;
        }
        SortKey::Tag(tag_lower)
    };
    Some((key, descending))
}

/// Sort songs in place by a resolved `sort` key, reversing for descending
/// order. Only the first value of a multi-valued tag is used (matches
/// MPD's documented behaviour).
pub fn sort_songs(songs: &mut [rmpd_core::song::Song], key: &SortKey, descending: bool) {
    match key {
        SortKey::LastModified => songs.sort_by_key(|s| s.last_modified),
        SortKey::Added => songs.sort_by_key(|s| s.added_at),
        SortKey::Tag(tag) => {
            songs.sort_by(|a, b| a.tag_with_fallback(tag).cmp(&b.tag_with_fallback(tag)))
        }
    }
    if descending {
        songs.reverse();
    }
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

/// Append `songs` to the queue, then — if `position` is before the queue's
/// prior end — move the newly-added block there in one pass. Mirrors MPD's
/// `handle_match_add` (`AddFromDatabase` followed by `Partition::MoveRange`
/// when the insert position isn't a plain append). `position` beyond the
/// queue's prior length is rejected with MPD's `"Bad song index"` instead of
/// silently clamping.
pub async fn add_songs_at_position(
    state: &crate::state::AppState,
    songs: Vec<rmpd_core::song::Song>,
    position: Option<u32>,
    command: &str,
) -> Result<(), String> {
    let start = state.queue.read().await.len() as u32;
    if let Some(pos) = position
        && pos > start
    {
        return Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "Bad song index",
        ));
    }
    if songs.is_empty() {
        return Ok(());
    }
    let added = songs.len() as u32;
    {
        let mut queue = state.queue.write().await;
        for song in songs {
            queue.add(song);
        }
    }
    if let Some(pos) = position
        && pos < start
    {
        let mut queue = state.queue.write().await;
        for i in 0..added {
            queue.move_item(start, pos + i);
        }
    }
    crate::helpers::update_playlist_version(state).await;
    Ok(())
}

/// Resolve the optional 0.24 `position` argument of `findadd`/`searchadd`/
/// `searchaddpl` (`+N`/`-N`/absolute — same grammar as `addid`) into an
/// absolute queue index, mirroring MPD's `ParseInsertPosition`
/// (`PositionArg.cxx`). `None` means "append" and passes through unchanged.
pub async fn resolve_add_position(
    state: &crate::state::AppState,
    spec: Option<crate::parser::InsertPosition>,
    command: &str,
) -> Result<Option<u32>, String> {
    use crate::parser::InsertPosition;

    let Some(spec) = spec else {
        return Ok(None);
    };
    let queue_len = state.queue.read().await.len() as u32;
    let resolved = match spec {
        InsertPosition::Absolute(n) => {
            if n > queue_len {
                return Err(ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    command,
                    &format!("Number too large: {n}"),
                ));
            }
            n
        }
        InsertPosition::After(n) | InsertPosition::Before(n) => {
            let current = match state.status.read().await.current_song {
                Some(pos) => pos.position,
                None => {
                    return Err(ResponseBuilder::error(
                        ACK_ERROR_PLAYER_SYNC,
                        0,
                        command,
                        "No current song",
                    ));
                }
            };
            if let InsertPosition::After(n) = spec {
                let max = queue_len - current - 1;
                if n > max {
                    return Err(ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        command,
                        &format!("Number too large: {n}"),
                    ));
                }
                current + 1 + n
            } else {
                if n > current {
                    return Err(ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        command,
                        &format!("Number too large: {n}"),
                    ));
                }
                current - n
            }
        }
    };
    Ok(Some(resolved))
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

/// Append the queue-item priority to the response (Prio, only if non-zero).
/// Range is emitted by `ResponseBuilder::song()` itself, positioned right
/// after the `file` field per MPD's SongPrint.cxx.
pub fn add_queue_item_metadata(
    resp: &mut crate::response::ResponseBuilder,
    item: &rmpd_core::queue::QueueItem,
) {
    if item.priority > 0 {
        resp.field("Prio", item.priority);
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
