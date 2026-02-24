//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_SYSTEM, format_iso8601_timestamp, open_db, song_tag_contains,
};

/// Parse an .m3u playlist file and return the list of relative paths.
/// Lines starting with '#' are comments and are skipped.
fn read_m3u_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(playlist_dir).join(format!("{name}.m3u"));
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("No such playlist: {e}"))?;
    let paths: Vec<String> = content
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(paths)
}

pub async fn handle_listplaylists_command(state: &AppState) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylists", "playlist directory not configured"),
    };

    let mut resp = ResponseBuilder::new();

    // Read .m3u files from playlist directory, matching MPD's filesystem-based approach
    let dir = match std::fs::read_dir(&playlist_dir) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylists", &format!("Error reading playlist directory: {e}"));
        }
    };

    let mut entries: Vec<(String, i64)> = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("m3u") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let mtime = entry.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                entries.push((stem.to_string(), mtime));
            }
        }
    }

    // Sort alphabetically to match MPD ordering
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, mtime) in &entries {
        resp.field("playlist", name);
        let timestamp_str = format_iso8601_timestamp(*mtime);
        resp.field("Last-Modified", &timestamp_str);
    }
            resp.ok()
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

    // MPD default: replace existing playlist (or create if new)
    let mode = mode.unwrap_or(SaveMode::Replace);

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
                if let Some(pos) = position {
                    // Insert at specified position
                    for (i, song) in songs.into_iter().enumerate() {
                        queue.add_at(song, Some(pos + i as u32));
                    }
                } else {
                    // MPD spec: load appends to queue
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
                );
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
                );
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
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylist", "playlist directory not configured"),
    };

    let paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylist", &e),
    };

    let total = paths.len();
    let (start, end) = if let Some((s, e)) = range {
        (s as usize, (e as usize).min(total))
    } else {
        (0, total)
    };
    let slice = &paths[start.min(total)..end.min(total)];

    let mut resp = ResponseBuilder::new();
    for path in slice {
        resp.field("file", path);
    }
    resp.ok()
}
pub async fn handle_listplaylistinfo_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylistinfo", "playlist directory not configured"),
    };

    let paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listplaylistinfo", &e),
    };
    let db = match open_db(state, "listplaylistinfo") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let total = paths.len();
    let (start, end) = if let Some((s, e)) = range {
        (s as usize, (e as usize).min(total))
    } else {
        (0, total)
    };
    let slice = &paths[start.min(total)..end.min(total)];

    let mut resp = ResponseBuilder::new();
    for path in slice {
        match db.find_songs("file", path) {
            Ok(songs) if !songs.is_empty() => {
                resp.song(&songs[0], None, None);
            }
            _ => {
                // Song not in DB — emit just the file path like MPD does for unknown tracks
                resp.field("file", path);
            }
        }
    }
    resp.ok()
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
            );
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
            );
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
