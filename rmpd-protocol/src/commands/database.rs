//! Database and library browsing command handlers

use tracing::{debug, error, info};

use crate::response::{Response, ResponseBuilder};
use crate::state::AppState;

/// Strip music directory prefix from absolute path
fn strip_music_dir_prefix<'a>(path: &'a str, music_dir: Option<&str>) -> &'a str {
    if let Some(music_dir) = music_dir {
        // Normalize music_dir to end with /
        let music_dir_with_slash = if music_dir.ends_with('/') {
            music_dir
        } else {
            // Need to handle this case by checking both variants
            if let Some(stripped) = path.strip_prefix(music_dir) {
                return stripped.trim_start_matches('/');
            }
            music_dir
        };

        if let Some(stripped) = path.strip_prefix(music_dir_with_slash) {
            return stripped;
        }
    }
    path
}

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

    // Calculate year starting from 1970
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

    // Calculate month and day
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

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Helper function to get tag value for sorting
fn get_tag_value(song: &rmpd_core::song::Song, tag: &str) -> String {
    match tag.to_lowercase().as_str() {
        "artist" => song.artist.clone().unwrap_or_default(),
        "album" => song.album.clone().unwrap_or_default(),
        "albumartist" => song.album_artist.clone().unwrap_or_default(),
        "title" => song.title.clone().unwrap_or_default(),
        "track" => song.track.map(|t| t.to_string()).unwrap_or_default(),
        "date" => song.date.clone().unwrap_or_default(),
        "genre" => song.genre.clone().unwrap_or_default(),
        "composer" => song.composer.clone().unwrap_or_default(),
        "performer" => song.performer.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

pub async fn handle_find_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "find", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("database error: {}", e)),
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "find", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e))
                }
            },
            Err(e) => {
                return ResponseBuilder::error(2, 0, "find", &format!("filter parse error: {}", e))
            }
        }
    } else if filters.len() == 1 {
        // Simple single tag/value search
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
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

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => return ResponseBuilder::error(50, 0, "find", &format!("query error: {}", e)),
        }
    };

    // Apply sorting if requested
    if let Some(sort_tag) = sort {
        songs.sort_by(|a, b| {
            let a_val = get_tag_value(a, sort_tag);
            let b_val = get_tag_value(b, sort_tag);
            a_val.cmp(&b_val)
        });
    }

    // Apply window filtering if requested
    let filtered = if let Some((start, end)) = window {
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

pub async fn handle_search_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "search", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "search", &format!("database error: {}", e))
        }
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "search", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
                }
            },
            Err(e) => {
                return ResponseBuilder::error(
                    2,
                    0,
                    "search",
                    &format!("filter parse error: {}", e),
                )
            }
        }
    } else if filters.len() == 1 {
        let tag = &filters[0].0;
        let value = &filters[0].1;

        if tag.eq_ignore_ascii_case("any") {
            // Use FTS for "any" tag
            match db.search_songs(value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("search error: {}", e))
                }
            }
        } else {
            // Partial match using LIKE
            match db.find_songs(tag, value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
                }
            }
        }
    } else {
        // Multiple tag/value pairs - build filter expression with AND
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

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "search", &format!("query error: {}", e))
            }
        }
    };

    // Apply sorting if requested
    if let Some(sort_tag) = sort {
        songs.sort_by(|a, b| {
            let a_val = get_tag_value(a, sort_tag);
            let b_val = get_tag_value(b, sort_tag);
            a_val.cmp(&b_val)
        });
    }

    // Apply window filtering if requested
    let filtered = if let Some((start, end)) = window {
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

pub async fn handle_list_command(
    state: &AppState,
    tag: &str,
    filter_tag: Option<&str>,
    filter_value: Option<&str>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "list", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("database error: {}", e)),
    };

    // If filter is provided, get filtered results
    let values = if let (Some(ft), Some(fv)) = (filter_tag, filter_value) {
        match db.list_filtered(tag, ft, fv) {
            Ok(v) => v,
            Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("query error: {}", e)),
        }
    } else {
        // No filter, list all values
        let result = match tag.to_lowercase().as_str() {
            "artist" => db.list_artists(),
            "album" => db.list_albums(),
            "albumartist" => db.list_album_artists(),
            "genre" => db.list_genres(),
            _ => return ResponseBuilder::error(2, 0, "list", &format!("unsupported tag: {}", tag)),
        };

        match result {
            Ok(v) => v,
            Err(e) => return ResponseBuilder::error(50, 0, "list", &format!("query error: {}", e)),
        }
    };

    let mut resp = ResponseBuilder::new();
    let tag_key = match tag.to_lowercase().as_str() {
        "artist" => "Artist",
        "album" => "Album",
        "albumartist" => "AlbumArtist",
        "genre" => "Genre",
        _ => tag,
    };

    for value in values {
        resp.field(tag_key, value);
    }
    resp.ok()
}

pub async fn handle_count_command(
    state: &AppState,
    filters: &[(String, String)],
    group: Option<&str>,
) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "count", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => return ResponseBuilder::error(50, 0, "count", &format!("database error: {}", e)),
    };

    if filters.is_empty() {
        return ResponseBuilder::error(2, 0, "count", "missing arguments");
    }

    // Get songs based on filters
    let songs = if filters.len() == 1 {
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "count", &format!("query error: {}", e))
            }
        }
    } else {
        // Multiple filters - build AND expression
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

        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "count", &format!("query error: {}", e))
            }
        }
    };

    let mut resp = ResponseBuilder::new();

    if let Some(group_tag) = group {
        // Group by specified tag
        use std::collections::HashMap;
        let mut groups: HashMap<String, (usize, u64)> = HashMap::new();

        for song in &songs {
            let group_value = get_tag_value(song, group_tag);
            let entry = groups.entry(group_value.clone()).or_insert((0, 0));
            entry.0 += 1;
            if let Some(duration) = song.duration {
                entry.1 += duration.as_secs();
            }
        }

        for (value, (count, playtime)) in groups {
            resp.field(group_tag, &value);
            resp.field("songs", count);
            resp.field("playtime", playtime);
        }
    } else {
        // No grouping - return totals
        let total_duration: u64 = songs
            .iter()
            .filter_map(|s| s.duration)
            .map(|d| d.as_secs())
            .sum();

        resp.field("songs", songs.len());
        resp.field("playtime", total_duration);
    }

    resp.ok()
}

pub async fn handle_update_command(state: &AppState, _path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p.clone(),
        None => return ResponseBuilder::error(50, 0, "update", "database not configured"),
    };

    let music_dir = match &state.music_dir {
        Some(p) => p.clone(),
        None => return ResponseBuilder::error(50, 0, "update", "music directory not configured"),
    };

    let event_bus = state.event_bus.clone();

    // Spawn background scanning task (blocking task since scan is synchronous)
    tokio::task::spawn_blocking(move || {
        info!("Starting library update");

        match rmpd_library::Database::open(&db_path) {
            Ok(db) => {
                let scanner = rmpd_library::Scanner::new(event_bus.clone());
                match scanner.scan_directory(&db, std::path::Path::new(&music_dir)) {
                    Ok(stats) => {
                        info!(
                            "Library scan complete: {} scanned, {} added, {} updated, {} errors",
                            stats.scanned, stats.added, stats.updated, stats.errors
                        );
                    }
                    Err(e) => {
                        error!("Library scan error: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to open database: {}", e);
            }
        }
    });

    // Return update job ID
    let mut resp = ResponseBuilder::new();
    resp.field("updating_db", 1);
    resp.ok()
}

pub async fn handle_albumart_command(state: &AppState, uri: &str, offset: usize) -> Response {
    info!("AlbumArt command: uri=[{}], offset={}", uri, offset);

    let db_path = match &state.db_path {
        Some(p) => p,
        None => {
            return Response::Text(ResponseBuilder::error(
                50,
                0,
                "albumart",
                "database not configured",
            ))
        }
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return Response::Text(ResponseBuilder::error(
                50,
                0,
                "albumart",
                &format!("database error: {}", e),
            ))
        }
    };

    // Resolve relative path to absolute path
    let absolute_path = if uri.starts_with('/') {
        // Already absolute
        debug!("Using absolute path: {}", uri);
        uri.to_string()
    } else {
        // Relative to music directory
        match &state.music_dir {
            Some(music_dir) => {
                let path = format!("{}/{}", music_dir, uri);
                debug!("Resolved relative path: {} -> {}", uri, path);
                path
            }
            None => {
                return Response::Text(ResponseBuilder::error(
                    50,
                    0,
                    "albumart",
                    "music directory not configured",
                ))
            }
        }
    };

    let extractor = rmpd_library::AlbumArtExtractor::new(db);

    // Pass both: relative URI for cache key, absolute path for file reading
    match extractor.get_artwork(uri, &absolute_path, offset) {
        Ok(Some(artwork)) => {
            // Binary response with proper format
            let mut resp = ResponseBuilder::new();
            resp.field("size", artwork.total_size);
            resp.field("type", &artwork.mime_type);
            resp.binary_field("binary", &artwork.data);
            Response::Binary(resp.to_binary_response())
        }
        Ok(None) => {
            // When offset is past the end of data, return OK (not an error)
            // This is the correct MPD protocol behavior for chunked transfers
            Response::Text(ResponseBuilder::new().ok())
        }
        Err(e) => Response::Text(ResponseBuilder::error(
            50,
            0,
            "albumart",
            &format!("Error: {}", e),
        )),
    }
}

pub async fn handle_readpicture_command(state: &AppState, uri: &str, offset: usize) -> Response {
    // readpicture is similar to albumart but returns any embedded picture
    // For now, we'll use the same implementation
    handle_albumart_command(state, uri, offset).await
}

// Queue inspection
pub async fn handle_currentsong_command(state: &AppState) -> String {
    let status = state.status.read().await;
    let queue = state.queue.read().await;

    if let Some(current) = status.current_song {
        if let Some(item) = queue.get(current.position) {
            let mut resp = ResponseBuilder::new();
            resp.song(&item.song, Some(current.position), Some(current.id));
            return resp.ok();
        }
    }

    // No current song
    ResponseBuilder::new().ok()
}

// Browsing commands
pub async fn handle_lsinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "lsinfo", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "lsinfo", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    // Get directory listing
    match db.list_directory(path_str) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            let music_dir = state.music_dir.as_deref();

            // List subdirectories first
            for dir in &listing.directories {
                let display_dir = strip_music_dir_prefix(dir, music_dir);
                resp.field("directory", display_dir);
            }

            // Then list songs
            for song in &listing.songs {
                // Create a modified song with stripped path for display
                let display_path = strip_music_dir_prefix(song.path.as_str(), music_dir);
                let mut display_song = song.clone();
                display_song.path = display_path.into();
                resp.song(&display_song, None, None);
            }

            // For root directory, also list playlists
            if path_str.is_empty() || path_str == "/" {
                if let Ok(playlists) = db.list_playlists() {
                    for playlist in &playlists {
                        resp.field("playlist", &playlist.name);
                        let timestamp_str = format_iso8601_timestamp(playlist.last_modified);
                        resp.field("Last-Modified", &timestamp_str);
                    }
                }
            }

            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "lsinfo", &format!("Error: {}", e)),
    }
}

pub async fn handle_listall_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listall", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listall", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    match db.list_directory_recursive(path_str) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.field("file", &song.path);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listall", &format!("Error: {}", e)),
    }
}

pub async fn handle_listallinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listallinfo", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listallinfo", &format!("database error: {}", e))
        }
    };

    let path_str = path.unwrap_or("");

    match db.list_directory_recursive(path_str) {
        Ok(songs) => {
            let mut resp = ResponseBuilder::new();
            for song in &songs {
                resp.song(song, None, None);
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listallinfo", &format!("Error: {}", e)),
    }
}

pub async fn handle_searchadd_command(state: &AppState, tag: &str, value: &str) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "searchadd", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "searchadd", &format!("database error: {}", e))
        }
    };

    // Search for songs
    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchadd", &format!("search error: {}", e))
            }
        }
    } else {
        match db.find_songs(tag, value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(50, 0, "searchadd", &format!("query error: {}", e))
            }
        }
    };

    // Add all found songs to queue
    for song in songs {
        state.queue.write().await.add(song);
    }

    // Update status
    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;

    ResponseBuilder::new().ok()
}

pub async fn handle_findadd_command(state: &AppState, tag: &str, value: &str) -> String {
    // findadd is exact match version of searchadd
    handle_searchadd_command(state, tag, value).await
}

pub async fn handle_listfiles_command(state: &AppState, uri: Option<&str>) -> String {
    let db_path = match &state.db_path {
        Some(p) => p,
        None => return ResponseBuilder::error(50, 0, "listfiles", "database not configured"),
    };

    let db = match rmpd_library::Database::open(db_path) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(50, 0, "listfiles", &format!("database error: {}", e))
        }
    };

    let path = uri.unwrap_or("");

    match db.list_directory(path) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            for dir in listing.directories {
                resp.field("directory", dir);
            }
            for song in listing.songs {
                resp.field("file", song.path.as_str());
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "listfiles", &format!("Error: {}", e)),
    }
}
