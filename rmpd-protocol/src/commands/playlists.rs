//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

/// Convert Unix timestamp to ISO 8601 format (RFC 3339)
fn format_iso8601_timestamp(timestamp: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const SECONDS_PER_HOUR: i64 = 3600;
    const SECONDS_PER_DAY: i64 = 86400;

    let mut days = timestamp / SECONDS_PER_DAY;
    let remaining = timestamp % SECONDS_PER_DAY;
    let hours = remaining / SECONDS_PER_HOUR;
    let minutes = (remaining % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = remaining % SECONDS_PER_MINUTE;

    let mut year = 1970;
    loop {
        let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap_year { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_month = if leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &dim in &days_in_month {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    let day = days + 1;

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

pub async fn handle_listplaylists_command(state: &AppState) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylists", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listplaylists", &format!("database error: {e}"))
        }
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
        Err(e) => ResponseBuilder::error(50, 0, "listplaylists", &format!("Error: {e}")),
    }
}

pub async fn handle_save_command(
    state: &AppState,
    name: &str,
    mode: Option<crate::parser::SaveMode>,
) -> String {
    use crate::parser::SaveMode;

    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "save", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "save", &format!("database error: {e}")),
    };

    // Get current queue
    let queue = state.queue.read().await;
    let songs: Vec<_> = queue.items().iter().map(|item| item.song.clone()).collect();
    drop(queue);

    // Handle different save modes
    let mode = mode.unwrap_or(SaveMode::Create);

    match mode {
        SaveMode::Create => {
            // Default: create new playlist or fail if exists
            // Check if playlist already exists
            match db.load_playlist(name) {
                Ok(_) => {
                    // Playlist exists, fail
                    ResponseBuilder::error(50, 0, "save", "Playlist already exists")
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {e}")),
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
                Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {e}")),
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
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {e}")),
                    }
                }
                Err(_) => {
                    // Playlist doesn't exist, create it
                    match db.save_playlist(name, &songs) {
                        Ok(_) => ResponseBuilder::new().ok(),
                        Err(e) => ResponseBuilder::error(50, 0, "save", &format!("Error: {e}")),
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
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "load", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "load", &format!("database error: {e}")),
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
                    return ResponseBuilder::error(50, 0, "load", "Invalid range");
                }
            }

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
            drop(queue);

            // Update status
            let mut status = state.status.write().await;
            status.playlist_version += 1;
            status.playlist_length = state.queue.read().await.len() as u32;

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "load", &format!("Error: {e}")),
    }
}

pub async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    // Search and add results to stored playlist
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "searchaddpl", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "searchaddpl", &format!("database error: {e}"))
        }
    };

    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchaddpl", &format!("search error: {e}"))
            }
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchaddpl", &format!("query error: {e}"))
            }
        }
    };

    for song in songs {
        if let Err(e) = db.playlist_add(name, song.path.as_str()) {
            return ResponseBuilder::error(50, 0, "searchaddpl", &format!("Error: {e}"));
        }
    }

    ResponseBuilder::new().ok()
}

pub async fn handle_listplaylist_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listplaylist", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listplaylist", &format!("database error: {e}"))
        }
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            // Apply range filter
            let filtered = if let Some((start, end)) = range {
                let start_idx = start as usize;
                let end_idx = end.min(songs.len() as u32) as usize;
                if start_idx < songs.len() {
                    &songs[start_idx..end_idx]
                } else {
                    &[]
                }
            } else {
                &songs[..]
            };

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylist", &format!("Error: {e}")),
    }
}

pub async fn handle_listplaylistinfo_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => {
            return ResponseBuilder::error(50, 0, "listplaylistinfo", "database not configured")
        }
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "listplaylistinfo",
                &format!("database error: {e}"),
            )
        }
    };

    match db.get_playlist_songs(name) {
        Ok(songs) => {
            // Apply range filter
            let filtered = if let Some((start, end)) = range {
                let start_idx = start as usize;
                let end_idx = end.min(songs.len() as u32) as usize;
                if start_idx < songs.len() {
                    &songs[start_idx..end_idx]
                } else {
                    &[]
                }
            } else {
                &songs[..]
            };

            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listplaylistinfo", &format!("Error: {e}")),
    }
}

pub async fn handle_playlistadd_command(
    state: &AppState,
    name: &str,
    uri: &str,
    position: Option<u32>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistadd", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistadd", &format!("database error: {e}"))
        }
    };

    // TODO: Implement position support in database layer
    // For now, position parameter is parsed but not used
    let _ = position;

    match db.playlist_add(name, uri) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistadd", &format!("Error: {e}")),
    }
}

pub async fn handle_playlistclear_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistclear", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistclear", &format!("database error: {e}"))
        }
    };

    match db.playlist_clear(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistclear", &format!("Error: {e}")),
    }
}

pub async fn handle_playlistdelete_command(state: &AppState, name: &str, position: u32) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistdelete", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistdelete", &format!("database error: {e}"))
        }
    };

    match db.playlist_delete_pos(name, position) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistdelete", &format!("Error: {e}")),
    }
}

pub async fn handle_playlistmove_command(
    state: &AppState,
    name: &str,
    from: u32,
    to: u32,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "playlistmove", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "playlistmove", &format!("database error: {e}"))
        }
    };

    match db.playlist_move(name, from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "playlistmove", &format!("Error: {e}")),
    }
}

pub async fn handle_rm_command(state: &AppState, name: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "rm", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "rm", &format!("database error: {e}")),
    };

    match db.delete_playlist(name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "rm", &format!("Error: {e}")),
    }
}

pub async fn handle_rename_command(state: &AppState, from: &str, to: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "rename", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "rename", &format!("database error: {e}")),
    };

    match db.rename_playlist(from, to) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(50, 0, "rename", &format!("Error: {e}")),
    }
}

pub async fn handle_playlistfind_command(state: &AppState, tag: &str, value: &str) -> String {
    // Search queue for exact matches
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

pub async fn handle_playlistsearch_command(state: &AppState, tag: &str, value: &str) -> String {
    // Case-insensitive search in queue
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

// Stored playlist search and utility commands
pub async fn handle_searchplaylist_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    // Search stored playlist for songs matching tag/value
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(songs) = db.load_playlist(name) {
                let mut resp = ResponseBuilder::new();
                let value_lower = value.to_lowercase();

                for song in songs {
                    let matches = match tag.to_lowercase().as_str() {
                        "artist" => song
                            .artist
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        "album" => song
                            .album
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        "title" => song
                            .title
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&value_lower))
                            .unwrap_or(false),
                        _ => false,
                    };

                    if matches {
                        resp.song(&song, None, None);
                    }
                }
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "searchplaylist", "Playlist not found")
}

pub async fn handle_playlistlength_command(state: &AppState, name: &str) -> String {
    // Get playlist length and total duration
    if let Some(ref db_path) = state.db_path {
        if let Ok(db) = rmpd_library::Database::open(db_path) {
            if let Ok(songs) = db.load_playlist(name) {
                let total_duration: f64 = songs
                    .iter()
                    .filter_map(|s| s.duration)
                    .map(|d| d.as_secs_f64())
                    .sum();

                let mut resp = ResponseBuilder::new();
                resp.field("songs", songs.len().to_string());
                resp.field("playtime", format!("{total_duration:.3}"));
                return resp.ok();
            }
        }
    }
    ResponseBuilder::error(50, 0, "playlistlength", "Playlist not found")
}
