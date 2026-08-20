//! Queue (current playlist) manipulation and inspection commands

use tracing::debug;

use crate::commands::playback;
use crate::helpers;
use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_NO_EXIST, ACK_ERROR_PERMISSION, ACK_ERROR_PLAYER_SYNC, ACK_ERROR_SYS,
    add_at_checked, add_queue_item_metadata, apply_range, open_db, prepare_song_for_playback,
    update_next_song,
};

fn number_too_large(command: &str, n: u32) -> String {
    ResponseBuilder::error(ACK_ERROR_ARG, 0, command, &format!("Number too large: {n}"))
}

/// Look up the position of the currently-playing/paused song, mirroring
/// MPD's `RequireCurrentPosition` (PositionArg.cxx). Errors when nothing is
/// current (e.g. an empty queue, or playback never started).
async fn require_current_position(state: &AppState, command: &str) -> Result<u32, String> {
    match state.status.read().await.current_song {
        Some(pos) => Ok(pos.position),
        None => Err(ResponseBuilder::error(
            ACK_ERROR_PLAYER_SYNC,
            0,
            command,
            "No current song",
        )),
    }
}

/// Resolve an `add`/`addid` POSITION argument (`+N`/`-N`/absolute) into an
/// absolute queue index, mirroring MPD's `ParseInsertPosition`
/// (PositionArg.cxx).
async fn resolve_insert_position(
    state: &AppState,
    spec: crate::parser::InsertPosition,
    command: &str,
) -> Result<u32, String> {
    use crate::parser::InsertPosition;

    let queue_len = state.queue.read().await.len() as u32;
    match spec {
        InsertPosition::Absolute(n) => {
            if n > queue_len {
                return Err(number_too_large(command, n));
            }
            Ok(n)
        }
        InsertPosition::After(n) => {
            let current = require_current_position(state, command).await?;
            let max = queue_len - current - 1;
            if n > max {
                return Err(number_too_large(command, n));
            }
            Ok(current + 1 + n)
        }
        InsertPosition::Before(n) => {
            let current = require_current_position(state, command).await?;
            if n > current {
                return Err(number_too_large(command, n));
            }
            Ok(current - n)
        }
    }
}

/// Resolve a `move`/`moveid` TO argument (`+N`/`-N`/absolute) into an
/// absolute queue index, mirroring MPD's `ParseMoveDestination`
/// (PositionArg.cxx). `range` is the half-open `[start, end)` source range
/// being moved; it must already be non-empty and within `[0, queue_len]`.
async fn resolve_move_destination(
    state: &AppState,
    to: crate::parser::InsertPosition,
    range: (u32, u32),
    queue_len: u32,
    command: &str,
) -> Result<u32, String> {
    use crate::parser::InsertPosition;

    let (start, end) = range;
    let count = i64::from(end - start);

    let n = match to {
        InsertPosition::Absolute(n) => {
            let max = i64::from(queue_len) - count;
            if i64::from(n) > max {
                return Err(number_too_large(command, n));
            }
            return Ok(n);
        }
        InsertPosition::After(n) | InsertPosition::Before(n) => n,
    };

    let mut current = i64::from(require_current_position(state, command).await?);
    if current >= i64::from(start) && current < i64::from(end) {
        return Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "Cannot move current song relative to itself",
        ));
    }
    if current >= i64::from(end) {
        current -= count;
    }
    let max = i64::from(queue_len) - current - count;
    if i64::from(n) > max {
        return Err(number_too_large(command, n));
    }
    let resolved = if matches!(to, InsertPosition::After(_)) {
        current + 1 + i64::from(n)
    } else {
        current - i64::from(n)
    };
    if resolved < 0 {
        return Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "Bad song index",
        ));
    }
    Ok(resolved as u32)
}
/// Move the half-open range `[start, end)` to destination `to` (already
/// resolved, absolute, and bounds-checked against `queue_len - (end -
/// start)`) by repeatedly moving individual items — mirrors MPD's
/// `Queue::MoveRange`. `to` is the range's final absolute start position
/// in the *resulting* queue (i.e. the moved block ends up occupying
/// `[to, to + range_size)`), not an index into the post-removal array.
/// Returns `false` if a move failed (shouldn't happen for validated inputs).
fn move_range_in_queue(queue: &mut rmpd_core::queue::Queue, start: u32, end: u32, to: u32) -> bool {
    let range_size = end - start;
    if to <= start {
        // Each move removes the item now sitting at `start + i` (the next
        // not-yet-moved member of the range — items already moved out no
        // longer occupy `start`) and reinserts it at `to + i`.
        for i in 0..range_size {
            if !queue.move_item(start + i, to + i) {
                return false;
            }
        }
    } else {
        // Moving forward: repeatedly pop the item still at `start` (every
        // prior pop shifts the next range member down into `start`) and
        // reinsert it at the block's final last slot, `to + range_size -
        // 1`; later reinsertions push earlier ones back to keep the whole
        // block landing at `[to, to + range_size)` in order.
        let target = to + range_size - 1;
        for _ in 0..range_size {
            if !queue.move_item(start, target) {
                return false;
            }
        }
    }
    true
}

/// Result of resolving `add`'s URI: a single song, or every song under a
/// directory (including the database root), added recursively in path
/// order — mirrors MPD's `LocateUri` + `AddFromDatabase`.
enum AddOutcome {
    Song(rmpd_core::song::Song),
    Directory(Vec<rmpd_core::song::Song>),
}

pub async fn handle_add_command(
    state: &AppState,
    uri: &str,
    position: Option<crate::parser::InsertPosition>,
) -> String {
    debug!("add command received with URI: [{}]", uri);
    let position = match position {
        Some(spec) => match resolve_insert_position(state, spec, "add").await {
            Ok(pos) => Some(pos),
            Err(resp) => return resp,
        },
        None => None,
    };
    // "add /" is malformed but kept for backwards compatibility: it adds
    // the whole database, same as "add ''" (QueueCommands.cxx::handle_add).
    let uri = if uri == "/" { "" } else { uri };
    // A `<scheme>://` URI is a network stream (radio): validate the scheme and
    // add a synthetic stream song. Mount-style source paths and local paths have
    // no `://`, so they skip this block and fall through to the DB lookup below.
    if let Some(scheme_end) = uri.find("://") {
        let scheme = &uri[..scheme_end];
        if !helpers::is_known_uri_scheme(scheme) {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "add", "Unsupported URI scheme");
        }
        if scheme != "file" {
            let stream_song = helpers::create_stream_song(uri);
            // `add` returns no Id (unlike `addid`) — MPD replies with bare OK.
            return match add_at_checked(state, stream_song, position, "add").await {
                Ok(_) => {
                    helpers::update_playlist_version(state).await;
                    ResponseBuilder::new().ok()
                }
                Err(resp) => resp,
            };
        }
    }
    // Resolve the URI on a blocking-pool thread (SQLite is sync): a single
    // song by exact path, or — if that fails — a directory (including the
    // root) added recursively. The closure returns either the outcome or
    // the fully formatted error response.
    let state_clone = state.clone();
    let uri_owned = uri.to_string();
    let outcome_result: Result<AddOutcome, String> = match tokio::task::spawn_blocking(move || {
        let db = open_db(&state_clone, "add")?;
        match db.get_song_by_path(&uri_owned) {
            Ok(Some(s)) => Ok(AddOutcome::Song(s)),
            Ok(None) => match db.list_directory(&uri_owned) {
                Ok(_) => match db.list_directory_recursive(&uri_owned) {
                    Ok(songs) => Ok(AddOutcome::Directory(songs)),
                    Err(e) => Err(ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        "add",
                        &format!("query error: {e}"),
                    )),
                },
                Err(_) => Err(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "add",
                    "No such directory",
                )),
            },
            Err(e) => Err(ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "add",
                &format!("query error: {e}"),
            )),
        }
    })
    .await
    {
        Ok(res) => res,
        Err(_) => {
            return ResponseBuilder::error(ACK_ERROR_SYS, 0, "add", "internal error");
        }
    };

    let outcome = match outcome_result {
        Ok(o) => o,
        Err(resp) => return resp,
    };

    // `add` returns no Id (unlike `addid`) — MPD replies with bare OK.
    match outcome {
        AddOutcome::Song(song) => match add_at_checked(state, song, position, "add").await {
            Ok(_) => {
                helpers::update_playlist_version(state).await;
                ResponseBuilder::new().ok()
            }
            Err(resp) => resp,
        },
        AddOutcome::Directory(songs) => {
            let mut queue = state.queue.write().await;
            let old_size = queue.len() as u32;
            for song in songs {
                queue.add(song);
            }
            let new_size = queue.len() as u32;
            // Mirror MPD: move the appended block to POSITION only when it
            // was requested and lands before where it was appended; a
            // failed move is ignored (best-effort, matches handle_add's
            // `catch (...) { /* ignore */ }`).
            if let Some(pos) = position
                && pos < old_size
                && new_size > old_size
            {
                move_range_in_queue(&mut queue, old_size, new_size, pos);
            }
            drop(queue);
            helpers::update_playlist_version(state).await;
            ResponseBuilder::new().ok()
        }
    }
}

pub async fn handle_clear_command(state: &AppState) -> String {
    state.queue.write().await.clear();
    state.engine.write().await.stop().await.ok();
    helpers::update_playlist_version(state).await;

    let mut status = state.status.write().await;
    status.current_song = None;
    status.next_song = None;

    ResponseBuilder::new().ok()
}

pub async fn handle_delete_command(
    state: &AppState,
    target: crate::parser::DeleteTarget,
) -> String {
    use crate::parser::DeleteTarget;

    // Normalize to a half-open [start, end) range — a bare position is
    // [pos, pos+1) — mirroring MPD's RangeArg::CheckClip semantics: only
    // `start > len` is an error; `start == len` (and any end beyond it)
    // clips down to an empty, silently-OK range.
    let (start, end) = match target {
        DeleteTarget::Position(pos) => (pos, pos.saturating_add(1)),
        DeleteTarget::Range(start, end) => (start, end),
    };

    let mut queue = state.queue.write().await;
    let len = queue.len() as u32;
    if start > len {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "delete", "Bad song index");
    }
    let end = end.min(len);
    if start >= end {
        // Empty range: no-op
        return ResponseBuilder::new().ok();
    }
    // Delete from highest to lowest to avoid position shifts
    for pos in (start..end).rev() {
        queue.delete(pos);
    }
    drop(queue);
    helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

pub async fn handle_addid_command(
    state: &AppState,
    uri: &str,
    position: Option<crate::parser::InsertPosition>,
) -> String {
    debug!("addid command received with URI: [{}]", uri);
    let position = match position {
        Some(spec) => match resolve_insert_position(state, spec, "addid").await {
            Ok(pos) => Some(pos),
            Err(resp) => return resp,
        },
        None => None,
    };
    // A `<scheme>://` URI is a network stream (radio): validate the scheme and
    // add a synthetic stream song. Mount-style source paths and local paths have
    // no `://`, so they skip this block and fall through to the DB lookup below.
    if let Some(scheme_end) = uri.find("://") {
        let scheme = &uri[..scheme_end];
        if !helpers::is_known_uri_scheme(scheme) {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "addid", "Unsupported URI scheme");
        }
        if scheme != "file" {
            let stream_song = helpers::create_stream_song(uri);
            return match add_at_checked(state, stream_song, position, "addid").await {
                Ok(id) => {
                    helpers::update_playlist_version(state).await;
                    let mut resp = ResponseBuilder::new();
                    resp.field("Id", id);
                    resp.ok()
                }
                Err(resp) => resp,
            };
        }
    }
    // Get song from database (file:// or relative path) — run the blocking
    // DB open + query on a blocking-pool thread so it never stalls the async
    // runtime. The closure returns either the resolved `Song` or the fully
    // formatted error response, matching the original inline error handling.
    let state_clone = state.clone();
    let uri_owned = uri.to_string();
    let song_result: Result<rmpd_core::song::Song, String> =
        match tokio::task::spawn_blocking(move || {
            let db = open_db(&state_clone, "addid")?;
            match db.get_song_by_path(&uri_owned) {
                Ok(Some(s)) => Ok(s),
                Ok(None) => Err(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "addid",
                    "No such song",
                )),
                Err(e) => Err(ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "addid",
                    &format!("query error: {e}"),
                )),
            }
        })
        .await
        {
            Ok(res) => res,
            Err(_) => {
                return ResponseBuilder::error(ACK_ERROR_SYS, 0, "addid", "internal error");
            }
        };

    let song = match song_result {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    // Add to queue at specific position
    match add_at_checked(state, song, position, "addid").await {
        Ok(id) => {
            helpers::update_playlist_version(state).await;
            let mut resp = ResponseBuilder::new();
            resp.field("Id", id);
            resp.ok()
        }
        Err(resp) => resp,
    }
}

pub async fn handle_deleteid_command(state: &AppState, id: u32) -> String {
    if state.queue.write().await.delete_id(id).is_some() {
        helpers::update_playlist_version(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "deleteid", "No such song")
    }
}

pub async fn handle_moveid_command(
    state: &AppState,
    id: u32,
    to: crate::parser::InsertPosition,
) -> String {
    let queue_len = state.queue.read().await.len() as u32;
    let position = match state.queue.read().await.get_by_id(id) {
        Some(item) => item.position,
        None => return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "moveid", "No such song"),
    };

    let to =
        match resolve_move_destination(state, to, (position, position + 1), queue_len, "moveid")
            .await
        {
            Ok(pos) => pos,
            Err(resp) => return resp,
        };

    if state.queue.write().await.move_by_id(id, to) {
        helpers::update_playlist_version(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "moveid", "No such song")
    }
}

pub async fn handle_move_command(
    state: &AppState,
    from: crate::parser::MoveFrom,
    to: crate::parser::InsertPosition,
) -> String {
    use crate::parser::MoveFrom;

    // Normalize to a half-open [start, end) range — a bare position is
    // [pos, pos+1) — mirroring MPD's RangeArg. `move` never accepts an
    // open-ended FROM range (QueueCommands.cxx::handle_move).
    let (start, end) = match from {
        MoveFrom::Position(pos) => (pos, pos.saturating_add(1)),
        MoveFrom::Range(start, end) => {
            if end == u32::MAX {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "move",
                    "Open-ended range not supported",
                );
            }
            (start, end)
        }
    };

    let queue_len = state.queue.read().await.len() as u32;
    if start >= end || end > queue_len {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "move", "Bad song index");
    }

    let to = match resolve_move_destination(state, to, (start, end), queue_len, "move").await {
        Ok(pos) => pos,
        Err(resp) => return resp,
    };

    let mut queue = state.queue.write().await;
    if !move_range_in_queue(&mut queue, start, end, to) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "move", "Bad song index");
    }

    drop(queue);
    helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

pub async fn handle_swap_command(state: &AppState, pos1: u32, pos2: u32) -> String {
    if state.queue.write().await.swap(pos1, pos2) {
        helpers::update_playlist_version(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_ARG, 0, "swap", "Bad song index")
    }
}

pub async fn handle_swapid_command(state: &AppState, id1: u32, id2: u32) -> String {
    if state.queue.write().await.swap_by_id(id1, id2) {
        helpers::update_playlist_version(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "swapid", "No such song")
    }
}

pub async fn handle_shuffle_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    let mut queue = state.queue.write().await;
    let len = queue.len() as u32;

    match range {
        Some((start, end)) => {
            if start > len {
                return ResponseBuilder::error(ACK_ERROR_ARG, 0, "shuffle", "Bad song index");
            }
            let end = end.min(len);
            if end.saturating_sub(start) >= 2 {
                queue.shuffle_range(start, end);
            } else {
                return ResponseBuilder::new().ok();
            }
        }
        None => {
            if len < 2 {
                return ResponseBuilder::new().ok();
            }
            queue.shuffle();
        }
    }
    drop(queue);
    helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

pub async fn handle_playlistid_command(state: &AppState, id: Option<u32>) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if let Some(song_id) = id {
        // Get specific song by ID
        if let Some(item) = queue.get_by_id(song_id) {
            resp.song(&item.song, Some(item.position), Some(item.id), item.range);
            add_queue_item_metadata(&mut resp, item);
        } else {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "playlistid", "No such song");
        }
    } else {
        // Get all songs with IDs
        for item in queue.items() {
            resp.song(&item.song, Some(item.position), Some(item.id), item.range);
            add_queue_item_metadata(&mut resp, item);
        }
    }

    resp.ok()
}

pub async fn handle_playlistinfo_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    let queue = state.queue.read().await;
    let items = queue.items();

    // MPD's CheckClip: only `start > length` is an error; `start == length`
    // (and any end beyond it) clips down to an empty, silently-OK range.
    if let Some((start, _)) = range
        && start > items.len() as u32
    {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistinfo", "Bad song index");
    }

    let mut resp = ResponseBuilder::new();
    let filtered = apply_range(items, range);

    for item in filtered {
        resp.song(&item.song, Some(item.position), Some(item.id), item.range);
        add_queue_item_metadata(&mut resp, item);
    }

    resp.ok()
}

pub async fn handle_playid_command(state: &AppState, id: Option<u32>) -> String {
    if let Some(song_id) = id {
        // Play specific song by ID
        let queue = state.queue.read().await;
        if let Some(item) = queue.get_by_id(song_id) {
            let song = (*item.song).clone();
            let position = item.position;
            let range = item.range;
            drop(queue);

            let playback_song = match prepare_song_for_playback(
                &song,
                state.music_dir.as_deref(),
                range,
                &state.sources,
            )
            .await
            {
                Ok(ps) => ps,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_NO_EXIST,
                        0,
                        "playid",
                        &format!("Cannot resolve song: {}", e),
                    );
                }
            };

            match state.engine.write().await.play(playback_song).await {
                Ok(_) => {
                    {
                        let mut status = state.status.write().await;
                        status.state = rmpd_core::state::PlayerState::Play;
                        status.elapsed = Some(std::time::Duration::ZERO);
                        status.duration = song.duration;
                        status.bitrate = song.bitrate;
                        status.audio_format = helpers::extract_audio_format(&song);
                        status.current_song = Some(rmpd_core::state::QueuePosition {
                            position,
                            id: song_id,
                        });

                        let queue = state.queue.read().await;
                        update_next_song(&mut status, &queue, position);
                    }

                    // Mirror `play`: notify the `player` idle subsystem so clients
                    // update their now-playing view and cover art.
                    state
                        .event_bus
                        .emit(rmpd_core::event::Event::PlayerStateChanged(
                            rmpd_core::state::PlayerState::Play,
                        ));
                    state
                        .event_bus
                        .emit(rmpd_core::event::Event::SongChanged(Some(song)));

                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "playid",
                    &format!("Playback error: {e}"),
                ),
            }
        } else {
            ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "playid", "No such song")
        }
    } else {
        // Resume playback (same as play with no args)
        playback::handle_play_command(state, None).await
    }
}

/// Set priority for songs in queue by position range
///
/// Sets the priority for all songs within the specified position ranges.
/// Priority is 0-255 where higher values have higher priority.
///
/// Mirrors MPD's `handle_prio`: ranges are applied one at a time (not
/// validated upfront), so an out-of-bounds range only aborts the *rest* of
/// the list — ranges already applied before it stay applied.
pub async fn handle_prio_command(state: &AppState, priority: u8, ranges: &[(u32, u32)]) -> String {
    let mut applied = false;
    let error = {
        let mut queue = state.queue.write().await;
        let len = queue.len() as u32;
        let mut error = None;
        for &(start, end) in ranges {
            if start > len {
                error = Some(ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "prio",
                    "Bad song index",
                ));
                break;
            }
            queue.set_priority_range(priority, &[(start, end)]);
            applied = true;
        }
        error
    };

    if applied {
        helpers::update_playlist_version(state).await;
    }
    error.unwrap_or_else(|| ResponseBuilder::new().ok())
}

/// Set priority for songs in queue by ID
///
/// Sets the priority for all songs with the specified IDs. Priority is
/// 0-255 where higher values have higher priority.
///
/// Mirrors MPD's `handle_prioid`: IDs are applied one at a time (not
/// validated upfront), so a nonexistent ID only aborts the *rest* of the
/// list — IDs already applied before it stay applied.
pub async fn handle_prioid_command(state: &AppState, priority: u8, ids: &[u32]) -> String {
    let mut applied = false;
    let error = {
        let mut queue = state.queue.write().await;
        let mut error = None;
        for &id in ids {
            if queue.get_by_id(id).is_none() {
                error = Some(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "prioid",
                    "No such song",
                ));
                break;
            }
            queue.set_priority_ids(priority, &[id]);
            applied = true;
        }
        error
    };

    if applied {
        helpers::update_playlist_version(state).await;
    }
    error.unwrap_or_else(|| ResponseBuilder::new().ok())
}

/// Set playback range for a song
///
/// Sets a playback range (start and end time in seconds) for a song, or
/// clears it entirely when `range` is `None` (a bare `rangeid ID :`).
/// Mirrors MPD's `SetSongIdRange`: the currently playing/paused song cannot
/// be manipulated this way.
pub async fn handle_rangeid_command(
    state: &AppState,
    id: u32,
    range: Option<(f64, f64)>,
) -> String {
    if let Some(current) = state.status.read().await.current_song
        && current.id == id
    {
        return ResponseBuilder::error(
            ACK_ERROR_PERMISSION,
            0,
            "rangeid",
            "Cannot edit the current song",
        );
    }

    let found = {
        let mut queue = state.queue.write().await;
        queue.set_range_by_id(id, range)
    };

    if found {
        helpers::update_playlist_version(state).await;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "rangeid", "No such song")
    }
}

/// Add a tag to a queue item
///
/// Adds a custom tag to a queue item. Mirrors MPD's `AddSongIdTag`: only
/// remote songs (a `scheme://` URI) may have tags edited — local/database
/// files are rejected.
pub async fn handle_addtagid_command(state: &AppState, id: u32, tag: &str, value: &str) -> String {
    let canonical = rmpd_core::song::canonical_tag_name(&tag.to_lowercase());
    if canonical == "Unknown" {
        return ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "addtagid",
            &format!("Unknown tag type: {tag}"),
        );
    }

    {
        let queue = state.queue.read().await;
        match queue.get_by_id(id) {
            Some(item) if item.song.path.as_str().contains("://") => {}
            Some(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_PERMISSION,
                    0,
                    "addtagid",
                    "Cannot edit tags of local file",
                );
            }
            None => {
                return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "addtagid", "No such song");
            }
        }
    }

    state
        .queue
        .write()
        .await
        .add_tag_by_id(id, canonical.to_string(), value.to_string());
    helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

/// Clear tags from a queue item
///
/// If tag is specified, clears only that tag. Otherwise clears all tags.
/// Mirrors MPD's `ClearSongIdTag`: only remote songs (a `scheme://` URI)
/// may have tags edited — local/database files are rejected.
pub async fn handle_cleartagid_command(state: &AppState, id: u32, tag: Option<&str>) -> String {
    // Normalize empty tag to None (parser may return Some("") for missing arg)
    let tag = tag.filter(|t| !t.is_empty());
    let canonical = match tag {
        Some(t) => {
            let c = rmpd_core::song::canonical_tag_name(&t.to_lowercase());
            if c == "Unknown" {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "cleartagid",
                    &format!("Unknown tag type: {t}"),
                );
            }
            Some(c)
        }
        None => None,
    };

    {
        let queue = state.queue.read().await;
        match queue.get_by_id(id) {
            Some(item) if item.song.path.as_str().contains("://") => {}
            Some(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_PERMISSION,
                    0,
                    "cleartagid",
                    "Cannot edit tags of local file",
                );
            }
            None => {
                return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "cleartagid", "No such song");
            }
        }
    }

    state.queue.write().await.clear_tags_by_id(id, canonical);
    helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

/// Return changes in queue since version
///
/// MPD protocol: version 0 means "give me current playlist"
/// Otherwise, return items if playlist has changed since given version
pub async fn handle_plchanges_command(
    state: &AppState,
    version: u32,
    range: Option<(u32, u32)>,
) -> String {
    let current_version = state.status.read().await.playlist_version;
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || current_version > version {
        let items = queue.items();
        let filtered = apply_range(items, range);

        for item in filtered {
            resp.song(&item.song, Some(item.position), Some(item.id), item.range);
            add_queue_item_metadata(&mut resp, item);
        }
    }
    resp.ok()
}

/// Return position/id changes since version
///
/// MPD protocol: version 0 means "give me current playlist"
/// Otherwise, return items if playlist has changed since given version
pub async fn handle_plchangesposid_command(
    state: &AppState,
    version: u32,
    range: Option<(u32, u32)>,
) -> String {
    let current_version = state.status.read().await.playlist_version;
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || current_version > version {
        let items = queue.items();
        let filtered = apply_range(items, range);

        for item in filtered {
            resp.field("cpos", item.position.to_string());
            resp.field("Id", item.id.to_string());
        }
    }
    resp.ok()
}

/// Evaluate a `FilterExpression` against a queue item's song and priority
/// in memory (no SQL involved — unlike the database `find`/`search`, which
/// go through `FilterExpression::to_sql`). Mirrors `SongFilter::Match` in
/// MPD's `song/Filter.cxx`.
fn filter_matches_item(
    expr: &rmpd_core::filter::FilterExpression,
    item: &rmpd_core::queue::QueueItem,
) -> bool {
    use rmpd_core::filter::FilterExpression;
    match expr {
        FilterExpression::Compare {
            tag,
            op,
            value,
            case_sensitive,
            negated,
        } => {
            let tag_lower = tag.to_lowercase();
            let matched = if tag_lower == "file" {
                compare_str(*op, value, item.song.path.as_str(), *case_sensitive)
            } else if tag_lower == "any" {
                item.song
                    .tags
                    .iter()
                    .any(|(_, v)| compare_str(*op, value, v, *case_sensitive))
            } else {
                let values = item.song.tag_values_with_fallback(&tag_lower);
                if value.is_empty() && *op == rmpd_core::filter::CompareOp::Equal {
                    values.iter().all(|v| v.is_empty())
                } else {
                    values
                        .iter()
                        .any(|v| compare_str(*op, value, v, *case_sensitive))
                }
            };
            matched != *negated
        }
        FilterExpression::Base(prefix) => {
            let path = item.song.path.as_str();
            if prefix.is_empty() {
                !path.is_empty()
            } else {
                path == prefix.as_str() || path.starts_with(&format!("{prefix}/"))
            }
        }
        FilterExpression::ModifiedSince(ts) => item.song.last_modified >= *ts,
        FilterExpression::AddedSince(ts) => item.song.added_at >= *ts,
        FilterExpression::AudioFormat {
            sample_rate,
            bits,
            channels,
        } => {
            item.song.sample_rate.is_some()
                && item.song.bits_per_sample.is_some()
                && item.song.channels.is_some()
                && sample_rate.is_none_or(|v| item.song.sample_rate == Some(v))
                && bits.is_none_or(|v| item.song.bits_per_sample == Some(v))
                && channels.is_none_or(|v| item.song.channels == Some(v))
        }
        // Unlike database songs (never queued, implicit priority 0), a
        // queue item's priority is real.
        FilterExpression::Priority(n) => item.priority == *n,
        FilterExpression::And(left, right) => {
            filter_matches_item(left, item) && filter_matches_item(right, item)
        }
        FilterExpression::Not(inner) => !filter_matches_item(inner, item),
    }
}

/// One string comparison, mirroring `compare_sql`'s semantics but evaluated
/// directly against an in-memory value instead of building SQL.
fn compare_str(
    op: rmpd_core::filter::CompareOp,
    pattern: &str,
    candidate: &str,
    case_sensitive: bool,
) -> bool {
    use rmpd_core::filter::CompareOp;
    match op {
        CompareOp::Equal if case_sensitive => candidate == pattern,
        CompareOp::Equal => candidate.eq_ignore_ascii_case(pattern),
        CompareOp::Contains if case_sensitive => candidate.contains(pattern),
        CompareOp::Contains => candidate.to_lowercase().contains(&pattern.to_lowercase()),
        CompareOp::StartsWith if case_sensitive => candidate.starts_with(pattern),
        CompareOp::StartsWith => candidate
            .to_lowercase()
            .starts_with(&pattern.to_lowercase()),
        CompareOp::Regex => {
            let built = if case_sensitive {
                regex::Regex::new(pattern)
            } else {
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
            };
            built.is_ok_and(|re| re.is_match(candidate))
        }
    }
}

/// Compare two songs by a resolved `sort` key, mirroring
/// `commands::utils::sort_songs`'s per-key logic but over item references
/// (queue items own `Arc<Song>`, so that helper's `&mut [Song]` signature
/// doesn't fit here without cloning).
fn sort_key_cmp(
    a: &rmpd_core::song::Song,
    b: &rmpd_core::song::Song,
    key: &super::utils::SortKey,
) -> std::cmp::Ordering {
    use super::utils::SortKey;
    match key {
        SortKey::LastModified => a.last_modified.cmp(&b.last_modified),
        SortKey::Added => a.added_at.cmp(&b.added_at),
        SortKey::Tag(tag) => a.tag_with_fallback(tag).cmp(&b.tag_with_fallback(tag)),
    }
}

/// Shared implementation of `playlistfind`/`playlistsearch`: parse the
/// filter (modern expression or legacy `TAG VALUE` pairs — the same
/// grammar as the database `find`/`search`), match it against the queue in
/// memory, apply the optional `sort`/`window`, and print like
/// `playlistinfo`/`playlistid` (Pos/Id/Prio/Range via
/// `add_queue_item_metadata`).
async fn handle_playlist_match(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    fold_case: bool,
    command: &str,
) -> String {
    let expr = match super::utils::parse_filter_args(filters, fold_case) {
        Ok(e) => e,
        Err(e) => return super::utils::filter_parse_ack(command, &e),
    };

    let queue = state.queue.read().await;
    let mut matched: Vec<&rmpd_core::queue::QueueItem> = queue
        .items()
        .iter()
        .filter(|item| filter_matches_item(&expr, item))
        .collect();

    if let Some(sort_arg) = sort {
        match super::utils::parse_sort_tag(sort_arg) {
            Some((key, descending)) => {
                matched.sort_by(|a, b| sort_key_cmp(&a.song, &b.song, &key));
                if descending {
                    matched.reverse();
                }
            }
            None => {
                return ResponseBuilder::error(ACK_ERROR_ARG, 0, command, "Unknown sort tag");
            }
        }
    }

    let mut resp = ResponseBuilder::new();
    for item in apply_range(&matched, window) {
        resp.song(&item.song, Some(item.position), Some(item.id), item.range);
        add_queue_item_metadata(&mut resp, item);
    }
    resp.ok()
}

/// Search the queue for songs matching a filter expression (case-sensitive).
pub async fn handle_playlistfind_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    handle_playlist_match(state, filters, sort, window, false, "playlistfind").await
}

/// Search the queue for songs matching a filter expression (case-insensitive).
pub async fn handle_playlistsearch_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    handle_playlist_match(state, filters, sort, window, true, "playlistsearch").await
}
