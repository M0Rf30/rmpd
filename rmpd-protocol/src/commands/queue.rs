//! Queue (current playlist) manipulation and inspection commands

use tracing::debug;

use crate::commands::playback;
use crate::response::ResponseBuilder;
use crate::state::AppState;

pub async fn handle_add_command(state: &AppState, uri: &str, position: Option<u32>) -> String {
    debug!("Add command received with URI: [{}]", uri);
    // Get song from database
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "add", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "add", &format!("database error: {e}")),
    };

    let song = match db.get_song_by_path(uri) {
        Ok(Some(s)) => s,
        Ok(None) => return ResponseBuilder::error(50, 0, "add", "song not found in database"),
        Err(e) => return ResponseBuilder::error(50, 0, "add", &format!("query error: {e}")),
    };

    // Add to queue at specified position or at end
    let id = state.queue.write().await.add_at(song, position);

    // Update status to reflect playlist changes
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;
    drop(status); // Release the lock

    let mut resp = ResponseBuilder::new();
    resp.field("Id", id);
    resp.ok()
}

pub async fn handle_clear_command(state: &AppState) -> String {
    state.queue.write().await.clear();
    state.engine.write().await.stop().await.ok();

    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = 0;
    status.current_song = None;
    status.next_song = None;

    ResponseBuilder::new().ok()
}

pub async fn handle_delete_command(
    state: &AppState,
    target: crate::parser::DeleteTarget,
) -> String {
    use crate::parser::DeleteTarget;

    match target {
        DeleteTarget::Position(position) => {
            if state.queue.write().await.delete(position).is_some() {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                status.playlist_length = state.queue.read().await.len() as u32;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "delete", "No such song")
            }
        }
        DeleteTarget::Range(start, end) => {
            // Delete songs in range [start, end) (exclusive end)
            let mut queue = state.queue.write().await;
            let mut deleted_count = 0;

            // Delete from highest to lowest to avoid position shifts
            for pos in (start..end).rev() {
                if queue.delete(pos).is_some() {
                    deleted_count += 1;
                }
            }

            if deleted_count > 0 {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                status.playlist_length = queue.len() as u32;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "delete", "No such songs in range")
            }
        }
    }
}

pub async fn handle_addid_command(state: &AppState, uri: &str, position: Option<u32>) -> String {
    debug!(
        "AddId command received with URI: [{}], position: {:?}",
        uri, position
    );

    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "addid", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "addid", &format!("database error: {e}")),
    };

    let song = match db.get_song_by_path(uri) {
        Ok(Some(s)) => s,
        Ok(None) => return ResponseBuilder::error(50, 0, "addid", "song not found in database"),
        Err(e) => return ResponseBuilder::error(50, 0, "addid", &format!("query error: {e}")),
    };

    // Add to queue at specific position
    let id = state.queue.write().await.add_at(song, position);

    let mut resp = ResponseBuilder::new();
    resp.field("Id", id);
    resp.ok()
}

pub async fn handle_deleteid_command(state: &AppState, id: u32) -> String {
    if state.queue.write().await.delete_id(id).is_some() {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        status.playlist_length = state.queue.read().await.len() as u32;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "deleteid", "No such song")
    }
}

pub async fn handle_moveid_command(state: &AppState, id: u32, to: u32) -> String {
    if state.queue.write().await.move_by_id(id, to) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "moveid", "No such song")
    }
}

pub async fn handle_move_command(
    state: &AppState,
    from: crate::parser::MoveFrom,
    to: u32,
) -> String {
    use crate::parser::MoveFrom;

    match from {
        MoveFrom::Position(from_pos) => {
            if state.queue.write().await.move_item(from_pos, to) {
                let mut status = state.status.write().await;
                status.playlist_version += 1;
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(50, 0, "move", "Invalid position")
            }
        }
        MoveFrom::Range(start, end) => {
            // Move range of songs [start, end) to position
            // MPD semantics: move each song individually to maintain order
            let mut queue = state.queue.write().await;

            if start >= end || start >= queue.len() as u32 {
                return ResponseBuilder::error(50, 0, "move", "Invalid range");
            }

            let range_size = end.saturating_sub(start);

            // Move songs one by one
            // If moving to a position before the range, move from start to end
            // If moving to a position after the range, move from end-1 to start
            if to <= start {
                // Moving up in the queue
                for i in 0..range_size.min(queue.len() as u32 - start) {
                    if !queue.move_item(start, to + i) {
                        return ResponseBuilder::error(50, 0, "move", "Invalid position");
                    }
                }
            } else {
                // Moving down in the queue
                let actual_end = end.min(queue.len() as u32);
                for _ in 0..(actual_end - start) {
                    if !queue.move_item(start, to.saturating_sub(1)) {
                        return ResponseBuilder::error(50, 0, "move", "Invalid position");
                    }
                }
            }

            let mut status = state.status.write().await;
            status.playlist_version += 1;
            ResponseBuilder::new().ok()
        }
    }
}

pub async fn handle_swap_command(state: &AppState, pos1: u32, pos2: u32) -> String {
    if state.queue.write().await.swap(pos1, pos2) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "swap", "Invalid position")
    }
}

pub async fn handle_swapid_command(state: &AppState, id1: u32, id2: u32) -> String {
    if state.queue.write().await.swap_by_id(id1, id2) {
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "swapid", "No such song")
    }
}

pub async fn handle_shuffle_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    if let Some((start, end)) = range {
        state.queue.write().await.shuffle_range(start, end);
    } else {
        state.queue.write().await.shuffle();
    }
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    ResponseBuilder::new().ok()
}

pub async fn handle_playlistid_command(state: &AppState, id: Option<u32>) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if let Some(song_id) = id {
        // Get specific song by ID
        if let Some(item) = queue.get_by_id(song_id) {
            resp.song(&item.song, Some(item.position), Some(item.id));
            if item.priority > 0 {
                resp.field("Prio", item.priority);
            }
            if let Some((start, end)) = item.range {
                resp.field("Range", format!("{start:.3}-{end:.3}"));
            }
        } else {
            return ResponseBuilder::error(50, 0, "playlistid", "No such song");
        }
    } else {
        // Get all songs with IDs
        for item in queue.items() {
            resp.song(&item.song, Some(item.position), Some(item.id));
            if item.priority > 0 {
                resp.field("Prio", item.priority);
            }
            if let Some((start, end)) = item.range {
                resp.field("Range", format!("{start:.3}-{end:.3}"));
            }
        }
    }

    resp.ok()
}

pub async fn handle_playlistinfo_command(state: &AppState, range: Option<(u32, u32)>) -> String {
    let queue = state.queue.read().await;
    let items = queue.items();
    let mut resp = ResponseBuilder::new();

    // Apply range filter
    let filtered = if let Some((start, end)) = range {
        let start_idx = start as usize;
        let end_idx = end.min(items.len() as u32) as usize;
        if start_idx < items.len() {
            &items[start_idx..end_idx]
        } else {
            &[]
        }
    } else {
        items
    };

    for item in filtered {
        resp.song(&item.song, Some(item.position), Some(item.id));
        if item.priority > 0 {
            resp.field("Prio", item.priority);
        }
        if let Some((start, end)) = item.range {
            resp.field("Range", format!("{start:.3}-{end:.3}"));
        }
    }

    resp.ok()
}

/// Resolve relative path to absolute path using music_directory
fn resolve_path(rel_path: &str, music_dir: Option<&str>) -> String {
    if rel_path.starts_with('/') {
        return rel_path.to_string();
    }

    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{music_dir}/{rel_path}")
    } else {
        rel_path.to_string()
    }
}

pub async fn handle_playid_command(state: &AppState, id: Option<u32>) -> String {
    if let Some(song_id) = id {
        // Play specific song by ID
        let queue = state.queue.read().await;
        if let Some(item) = queue.get_by_id(song_id) {
            let song = item.song.clone();
            let position = item.position;
            drop(queue);

            // Resolve relative path to absolute for playback
            let mut playback_song = song.clone();
            let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
            playback_song.path = absolute_path.into();

            match state.engine.write().await.play(playback_song).await {
                Ok(_) => {
                    let mut status = state.status.write().await;
                    status.state = rmpd_core::state::PlayerState::Play;
                    status.current_song = Some(rmpd_core::state::QueuePosition {
                        position,
                        id: song_id,
                    });

                    // Set next_song for UI (e.g., Cantata's next button)
                    let queue = state.queue.read().await;
                    if let Some(next_item) = queue.get(position + 1) {
                        status.next_song = Some(rmpd_core::state::QueuePosition {
                            position: position + 1,
                            id: next_item.id,
                        });
                    } else {
                        status.next_song = None;
                    }

                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "playid", &format!("Playback error: {e}")),
            }
        } else {
            ResponseBuilder::error(50, 0, "playid", "No such song")
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
pub async fn handle_prio_command(state: &AppState, priority: u8, ranges: &[(u32, u32)]) -> String {
    let mut queue = state.queue.write().await;
    queue.set_priority_range(priority, ranges);

    let mut status = state.status.write().await;
    status.playlist_version = queue.version();

    ResponseBuilder::new().ok()
}

/// Set priority for songs in queue by ID
///
/// Sets the priority for all songs with the specified IDs.
/// Priority is 0-255 where higher values have higher priority.
pub async fn handle_prioid_command(state: &AppState, priority: u8, ids: &[u32]) -> String {
    let mut queue = state.queue.write().await;
    let changed = queue.set_priority_ids(priority, ids);

    if changed {
        let mut status = state.status.write().await;
        status.playlist_version = queue.version();
    }

    ResponseBuilder::new().ok()
}

/// Set playback range for a song
///
/// Sets a playback range (start and end time in seconds) for a song.
pub async fn handle_rangeid_command(state: &AppState, id: u32, range: (f64, f64)) -> String {
    let mut queue = state.queue.write().await;

    if queue.set_range_by_id(id, Some(range)) {
        drop(queue);
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "rangeid", "No such song")
    }
}

/// Add a tag to a queue item
///
/// Adds a custom tag to a queue item.
pub async fn handle_addtagid_command(state: &AppState, id: u32, tag: &str, value: &str) -> String {
    let mut queue = state.queue.write().await;

    if queue.add_tag_by_id(id, tag.to_string(), value.to_string()) {
        drop(queue);
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "addtagid", "No such song")
    }
}

/// Clear tags from a queue item
///
/// If tag is specified, clears only that tag. Otherwise clears all tags.
pub async fn handle_cleartagid_command(state: &AppState, id: u32, tag: Option<&str>) -> String {
    let mut queue = state.queue.write().await;

    if queue.clear_tags_by_id(id, tag) {
        drop(queue);
        let mut status = state.status.write().await;
        status.playlist_version += 1;
        ResponseBuilder::new().ok()
    } else {
        ResponseBuilder::error(50, 0, "cleartagid", "No such song")
    }
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
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || queue.version() > version {
        let items = queue.items();

        // Apply range filter
        let filtered = if let Some((start, end)) = range {
            let start_idx = start as usize;
            let end_idx = end.min(items.len() as u32) as usize;
            if start_idx < items.len() {
                &items[start_idx..end_idx]
            } else {
                &[]
            }
        } else {
            items
        };

        for item in filtered {
            resp.field("file", item.song.path.as_str());
            resp.field("Pos", item.position.to_string());
            resp.field("Id", item.id.to_string());
            if item.priority > 0 {
                resp.field("Prio", item.priority);
            }
            if let Some(ref title) = item.song.title {
                resp.field("Title", title);
            }
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
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    if version == 0 || queue.version() > version {
        let items = queue.items();

        // Apply range filter
        let filtered = if let Some((start, end)) = range {
            let start_idx = start as usize;
            let end_idx = end.min(items.len() as u32) as usize;
            if start_idx < items.len() {
                &items[start_idx..end_idx]
            } else {
                &[]
            }
        } else {
            items
        };

        for item in filtered {
            resp.field("cpos", item.position.to_string());
            resp.field("Id", item.id.to_string());
        }
    }
    resp.ok()
}

/// Search queue for exact tag matches
pub async fn handle_playlistfind_command(state: &AppState, tag: &str, value: &str) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();

    for item in queue.items() {
        let matches = match tag.to_lowercase().as_str() {
            "artist" => item.song.artist.as_deref() == Some(value),
            "album" => item.song.album.as_deref() == Some(value),
            "title" => item.song.title.as_deref() == Some(value),
            "genre" => item.song.genre.as_deref() == Some(value),
            _ => false,
        };

        if matches {
            resp.song(&item.song, Some(item.position), Some(item.id));
            if item.priority > 0 {
                resp.field("Prio", item.priority);
            }
            if let Some((start, end)) = item.range {
                resp.field("Range", format!("{start:.3}-{end:.3}"));
            }
        }
    }
    resp.ok()
}

/// Case-insensitive search in queue
pub async fn handle_playlistsearch_command(state: &AppState, tag: &str, value: &str) -> String {
    let queue = state.queue.read().await;
    let mut resp = ResponseBuilder::new();
    let value_lower = value.to_lowercase();

    for item in queue.items() {
        let matches = match tag.to_lowercase().as_str() {
            "artist" => item
                .song
                .artist
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "album" => item
                .song
                .album
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "title" => item
                .song
                .title
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            "genre" => item
                .song
                .genre
                .as_ref()
                .map(|s| s.to_lowercase().contains(&value_lower))
                .unwrap_or(false),
            _ => false,
        };

        if matches {
            resp.song(&item.song, Some(item.position), Some(item.id));
            if item.priority > 0 {
                resp.field("Prio", item.priority);
            }
            if let Some((start, end)) = item.range {
                resp.field("Range", format!("{start:.3}-{end:.3}"));
            }
        }
    }
    resp.ok()
}
