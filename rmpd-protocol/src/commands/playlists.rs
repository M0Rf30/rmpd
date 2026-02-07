//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    apply_range, format_iso8601_timestamp, open_db, song_tag_contains, ACK_ERROR_SYSTEM,
};

pub async fn handle_listplaylists_command(state: &AppState) -> String {
    let db = match open_db(state, "listplaylists") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.list_playlists() {
        Ok(playlists) => {
            let mut resp = ResponseBuilder::new();
            for playlist in &playlists {
                resp.field("playlist", &playlist.name);
                let timestamp_str = format_iso8601_timestamp(playlist.last_modified);
                resp.field("Last-Modified", &timestamp_str);
            }
            resp.ok()
        }
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylists", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_save_command(
    state: &AppState,
    name: &str,
    mode: Option<crate::parser::SaveMode>,
) -> String {
    use crate::parser::SaveMode;

    let db = match open_db(state, "save") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Get current queue
    let songs: Vec<_> = {
        let queue = state.queue.read().await;
        queue.items().iter().map(|item| item.song.clone()).collect()
    };

    // Handle different save modes
    let mode = mode.unwrap_or(SaveMode::Create);

    match mode {
        SaveMode::Create => {
            // Default: create new playlist or fail if exists
            // Check if playlist already exists
            match db.load_playlist(name) {
                Ok(_) => {
                    // Playlist exists, fail
                    ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "save", "Playlist already exists")
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(
                            ACK_ERROR_SYSTEM,
                            0,
                            "save",
                            &format!("Error: {e}"),
                        ),
                    }
                }
            }
        }
        SaveMode::Replace => {
            // Replace existing playlist or create if doesn't exist
            // Delete existing playlist if it exists (ignore errors)
            let _ = db.delete_playlist(name);

            // Save new playlist
            match db.save_playlist(name, &songs) {
                Ok(_) => ResponseBuilder::new().ok(),
                Err(e) => {
                    ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "save", &format!("Error: {e}"))
                }
            }
        }
        SaveMode::Append => {
            // Append to existing playlist or create if doesn't exist
            match db.load_playlist(name) {
                Ok(mut existing_songs) => {
                    // Playlist exists, append to it
                    existing_songs.extend(songs);

                    // Save updated playlist
                    match db.save_playlist(name, &existing_songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(
                            ACK_ERROR_SYSTEM,
                            0,
                            "save",
                            &format!("Error: {e}"),
                        ),
                    }
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(
                            ACK_ERROR_SYSTEM,
                            0,
                            "save",
                            &format!("Error: {e}"),
                        ),
                    }
                }
            }
        }
    }
}

pub async fn handle_load_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    let db = match open_db(state, "load") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.load_playlist(name) {
        Ok(mut songs) => {
            // Apply range filter if specified
            if let Some((start, end)) = range {
                let start = start as usize;
                let end = end.min(songs.len() as u32) as usize;
                if start < songs.len() {
                    songs = songs[start..end].to_vec();
                } else {
                    return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "load", "Invalid range");
                }
            }

            {
                let mut queue = state.queue.write().await;
                // If position is specified, add at that position
                // Otherwise, clear queue and add all songs
                if let Some(pos) = position {
                    for (i, song) in songs.into_iter().enumerate() {
                        queue.add_at(song, Some(pos + i as u32));
                    }
                } else {
                    queue.clear();
                    for song in songs {
                        queue.add(song);
                    }
                }
            }

            // Update status
            let mut status = state.status.write().await;
            status.playlist_version += 1;
            status.playlist_length = state.queue.read().await.len() as u32;

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "load", &format!("Error: {e}")),
    }
}

pub async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    // Search and add results to stored playlist
    let db = match open_db(state, "searchaddpl") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "searchaddpl",
                    &format!("search error: {e}"),
                )
            }
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "searchaddpl",
                    &format!("query error: {e}"),
                )
            }
        }
    };

    for song in songs {
        if let Err(e) = db.playlist_add(name, song.path.as_str()) {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "searchaddpl",
                &format!("Error: {e}"),
            );
        }
    }

    ResponseBuilder::new().ok()
}

pub async fn handle_listplaylist_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db = match open_db(state, "listplaylist") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            let filtered = apply_range(&songs, range);

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylist", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_listplaylistinfo_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db = match open_db(state, "listplaylistinfo") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            let filtered = apply_range(&songs, range);

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "listplaylistinfo",
            &format!("Error: {e}"),
        ),
    }
}

pub async fn handle_playlistadd_command(
    state: &AppState,
    name: &str,
    uri: &str,
    position: Option<u32>,
) -> String {
    let db = match open_db(state, "playlistadd") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // TODO: Implement position support in database layer
    // For now, position parameter is parsed but not used
    let _ = position;

    match db.playlist_add(name, uri) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistadd", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_playlistclear_command(state: &AppState, name: &str) -> String {
    let db = match open_db(state, "playlistclear") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.playlist_clear(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistclear", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_playlistdelete_command(state: &AppState, name: &str, position: u32) -> String {
    let db = match open_db(state, "playlistdelete") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.playlist_delete_pos(name, position) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "playlistdelete",
            &format!("Error: {e}"),
        ),
    }
}

pub async fn handle_playlistmove_command(
    state: &AppState,
    name: &str,
    from: u32,
    to: u32,
) -> String {
    let db = match open_db(state, "playlistmove") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.playlist_move(name, from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistmove", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_rm_command(state: &AppState, name: &str) -> String {
    let db = match open_db(state, "rm") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.delete_playlist(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "rm", &format!("Error: {e}")),
    }
}

pub async fn handle_rename_command(state: &AppState, from: &str, to: &str) -> String {
    let db = match open_db(state, "rename") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.rename_playlist(from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "rename", &format!("Error: {e}")),
    }
}

// Stored playlist search and utility commands
pub async fn handle_searchplaylist_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    let db = match open_db(state, "searchplaylist") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let songs = match db.load_playlist(name) {
        Ok(s) => s,
        Err(_) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "searchplaylist",
                "Playlist not found",
            )
        }
    };

    let mut resp = ResponseBuilder::new();
    let value_lower = value.to_lowercase();
    let tag_lower = tag.to_lowercase();

    for song in songs {
        if song_tag_contains(&song, &tag_lower, &value_lower) {
            resp.song(&song, None, None);
        }
    }
    resp.ok()
}

pub async fn handle_playlistlength_command(state: &AppState, name: &str) -> String {
    let db = match open_db(state, "playlistlength") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let songs = match db.load_playlist(name) {
        Ok(s) => s,
        Err(_) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistlength",
                "Playlist not found",
            )
        }
    };

    let total_duration: f64 = songs
        .iter()
        .filter_map(|s| s.duration)
        .map(|d| d.as_secs_f64())
        .sum();

    let mut resp = ResponseBuilder::new();
    resp.field("songs", songs.len().to_string());
    resp.field("playtime", format!("{total_duration:.3}"));
    resp.ok()
}
