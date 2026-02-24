//! Database and library browsing command handlers

use tracing::{error, info};

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

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_SYSTEM, apply_range, build_and_filter, format_iso8601_timestamp,
    open_db,
};

/// Helper function to get tag value with MPD-style fallback.
/// Delegates to Song::tag_with_fallback() for the normalized tag storage.
fn get_tag_value<'a>(song: &'a rmpd_core::song::Song, tag: &str) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    Cow::Borrowed(song.tag_with_fallback(tag).unwrap_or_default())
}

pub async fn handle_find_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    let db = match open_db(state, "find") {
        Ok(d) => d,
        Err(e) => return e,
    };

    if filters.is_empty() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "find", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "find",
                        &format!("query error: {e}"),
                    );
                }
            },
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "find",
                    &format!("filter parse error: {e}"),
                );
            }
        }
    } else if filters.len() == 1 {
        // Simple single tag/value search
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "find",
                    &format!("query error: {e}"),
                );
            }
        }
    } else {
        let expr = build_and_filter(filters);
        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "find",
                    &format!("query error: {e}"),
                );
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

    let filtered = apply_range(&songs, window);

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
    let db = match open_db(state, "search") {
        Ok(d) => d,
        Err(e) => return e,
    };

    if filters.is_empty() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "search", "missing arguments");
    }

    // Check if this is a filter expression (starts with '(')
    let mut songs = if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "search",
                        &format!("query error: {e}"),
                    );
                }
            },
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "search",
                    &format!("filter parse error: {e}"),
                );
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
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "search",
                        &format!("search error: {e}"),
                    );
                }
            }
        } else {
            // Partial match using LIKE
            match db.find_songs(tag, value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "search",
                        &format!("query error: {e}"),
                    );
                }
            }
        }
    } else {
        let expr = build_and_filter(filters);
        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "search",
                    &format!("query error: {e}"),
                );
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

    let filtered = apply_range(&songs, window);

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
    group: Option<&str>,
) -> String {
    let db = match open_db(state, "list") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // For grouped queries we need the full song list to extract both the group tag
    // and the requested tag. For non-grouped queries we can use the optimised path.
    if let Some(group_tag) = group {
        // Grouped: get all matching songs, then group by group_tag
        let songs = if let Some(ft) = filter_tag {
            if ft.starts_with('(') {
                match rmpd_core::filter::FilterExpression::parse(ft) {
                    Ok(filter) => match db.find_songs_filter(&filter) {
                        Ok(s) => s,
                        Err(e) => {
                            return ResponseBuilder::error(
                                ACK_ERROR_SYSTEM, 0, "list",
                                &format!("query error: {e}"),
                            );
                        }
                    },
                    Err(e) => {
                        return ResponseBuilder::error(
                            ACK_ERROR_ARG, 0, "list",
                            &format!("filter parse error: {e}"),
                        );
                    }
                }
            } else if let Some(fv) = filter_value {
                match db.find_songs(ft, fv) {
                    Ok(s) => s,
                    Err(e) => {
                        return ResponseBuilder::error(
                            ACK_ERROR_SYSTEM, 0, "list",
                            &format!("query error: {e}"),
                        );
                    }
                }
            } else {
                return ResponseBuilder::error(ACK_ERROR_ARG, 0, "list", "missing filter value");
            }
        } else {
            match db.get_all_songs() {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM, 0, "list",
                        &format!("query error: {e}"),
                    );
                }
            }
        };

        // Build map: group_value -> BTreeSet<tag_value> (sorted set)
        // Group values are sorted by MPD's std::map order (lexicographic)
        let mut groups: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
            std::collections::BTreeMap::new();

        let group_tag_lower = group_tag.to_lowercase();
        let tag_lower = tag.to_lowercase();
        for song in &songs {
            let group_vals = song.tag_values_with_fallback(&group_tag_lower);
            let tag_vals = song.tag_values_with_fallback(&tag_lower);

            let group_vals: Vec<&str> = if group_vals.is_empty() { vec![""] } else { group_vals };

            for gv in &group_vals {
                let tag_set = groups.entry(gv.to_string()).or_default();
                if tag_vals.is_empty() {
                    tag_set.insert(String::new());
                } else {
                    for tv in &tag_vals {
                        tag_set.insert(tv.to_string());
                    }
                }
            }
        }

        let group_key = rmpd_core::song::canonical_tag_name(&group_tag_lower);
        let tag_key = rmpd_core::song::canonical_tag_name(&tag_lower);

        let mut resp = ResponseBuilder::new();
        for (group_val, tag_vals) in &groups {
            resp.field(group_key, group_val);
            for tv in tag_vals {
                resp.field(tag_key, tv);
            }
        }
        return resp.ok();
    }

    // Non-grouped path (original logic)
    let values = if let Some(ft) = filter_tag {
        if ft.starts_with('(') {
            // Filter expression
            match rmpd_core::filter::FilterExpression::parse(ft) {
                Ok(filter) => match db.find_songs_filter(&filter) {
                    Ok(songs) => {
                        // Extract unique values of the requested tag
                        let mut seen = std::collections::BTreeSet::new();
                        for song in &songs {
                            let vals = song.tag_values_with_fallback(tag);
                            for val in vals {
                                if !val.is_empty() {
                                    seen.insert(val.to_string());
                                }
                            }
                        }
                        seen.into_iter().collect()
                    }
                    Err(e) => {
                        return ResponseBuilder::error(
                            ACK_ERROR_SYSTEM,
                            0,
                            "list",
                            &format!("query error: {e}"),
                        );
                    }
                },
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "list",
                        &format!("filter parse error: {e}"),
                    );
                }
            }
        } else if let Some(fv) = filter_value {
            // Traditional tag/value filter
            match db.list_filtered(tag, ft, fv) {
                Ok(v) => v,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "list",
                        &format!("query error: {e}"),
                    );
                }
            }
        } else {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "list", "missing filter value");
        }
    } else {
        // No filter, list all values using generic tag query
        let result = db.list_tag_values(tag);
        match result {
            Ok(v) => v,
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "list",
                    &format!("unsupported tag: {tag}"),
                );
            }
        }
    };

    let mut resp = ResponseBuilder::new();
    let tag_key = rmpd_core::song::canonical_tag_name(&tag.to_lowercase());
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
    let db = match open_db(state, "count") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Bare "count" with no args or bare tag without value (e.g. "count Genre") should error.
    // But "count group <tag>" (empty filters with group) is valid: count all songs grouped by tag.
    if filters.is_empty() && group.is_none() {
        return ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "count",
            "too few arguments for \"count\"",
        );
    }
    // Bare tag without value (e.g. "count Genre") should error like MPD
    if !filters.is_empty()
        && !filters[0].0.starts_with('(')
        && filters.len() == 1
        && filters[0].1.is_empty()
    {
        return ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "count",
            "too few arguments for \"count\"",
        );
    }

    // Get songs based on filters (empty filters = all songs)
    let songs = if filters.is_empty() {
        // No filter - count all songs (used with "count group <tag>")
        match db.get_all_songs() {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "count",
                    &format!("query error: {e}"),
                );
            }
        }
    } else if filters[0].0.starts_with('(') {
        // Parse as filter expression
        match rmpd_core::filter::FilterExpression::parse(&filters[0].0) {
            Ok(filter) => match db.find_songs_filter(&filter) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYSTEM,
                        0,
                        "count",
                        &format!("query error: {e}"),
                    );
                }
            },
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "count",
                    &format!("filter parse error: {e}"),
                );
            }
        }
    } else if filters.len() == 1 {
        match db.find_songs(&filters[0].0, &filters[0].1) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "count",
                    &format!("query error: {e}"),
                );
            }
        }
    } else {
        let expr = build_and_filter(filters);
        match db.find_songs_filter(&expr) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "count",
                    &format!("query error: {e}"),
                );
            }
        }
    };

    let mut resp = ResponseBuilder::new();

    if let Some(group_tag) = group {
        // Group by specified tag — sorted output to match MPD
        use std::collections::HashMap;
        let mut groups: HashMap<String, (usize, u64)> = HashMap::new();
        for song in &songs {
            let vals = song.tag_values_with_fallback(group_tag);
            let vals: Vec<&str> = if vals.is_empty() { vec![""] } else { vals };
            for group_value in vals {
                let entry = groups.entry(group_value.to_string()).or_insert((0, 0));
                entry.0 += 1;
                if let Some(duration) = song.duration {
                    entry.1 += duration.as_secs();
                }
            }
        }

        // Sort by tag value (MPD uses std::map which sorts lexicographically)
        let mut sorted: Vec<_> = groups.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let tag_key = rmpd_core::song::canonical_tag_name(&group_tag.to_lowercase());
        for (value, (count, playtime)) in &sorted {
            resp.field(tag_key, value);
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
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "update",
                "database not configured",
            );
        }
    };

    let music_dir = match &state.music_dir {
        Some(p) => p.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "update",
                "music directory not configured",
            );
        }
    };

    let event_bus = state.event_bus.clone();

    // Spawn background scanning task (blocking task since scan is synchronous)
    tokio::task::spawn_blocking(move || {
        info!("starting library update");

        match rmpd_library::Database::open(&db_path) {
            Ok(db) => {
                let scanner = rmpd_library::Scanner::new(event_bus.clone());
                match scanner.scan_directory(&db, std::path::Path::new(&music_dir)) {
                    Ok(stats) => {
                        info!(
                            "library scan complete: {} scanned, {} added, {} updated, {} errors",
                            stats.scanned, stats.added, stats.updated, stats.errors
                        );
                    }
                    Err(e) => {
                        error!("library scan error: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("failed to open database: {}", e);
            }
        }
    });

    // Return update job ID
    let mut resp = ResponseBuilder::new();
    resp.field("updating_db", 1);
    resp.ok()
}

pub async fn handle_albumart_command(state: &AppState, uri: &str, offset: usize) -> Response {
    info!("albumart command: uri=[{}], offset={}", uri, offset);

    let db = match open_db(state, "albumart") {
        Ok(d) => d,
        Err(e) => return Response::Text(e),
    };

    // Resolve relative path to absolute path
    let absolute_path = if uri.starts_with('/') {
        // Already absolute
        uri.to_string()
    } else {
        // Relative to music directory
        match &state.music_dir {
            Some(music_dir) => {
                let path = format!("{music_dir}/{uri}");
                path
            }
            None => {
                return Response::Text(ResponseBuilder::error(
                    50,
                    0,
                    "albumart",
                    "music directory not configured",
                ));
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
            &format!("Error: {e}"),
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

    if let Some(current) = status.current_song
        && let Some(item) = queue.get(current.position)
    {
        let mut resp = ResponseBuilder::new();
        resp.song(&item.song, Some(current.position), Some(current.id));
        return resp.ok();
    }

    // No current song
    ResponseBuilder::new().ok()
}

// Browsing commands
pub async fn handle_lsinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db = match open_db(state, "lsinfo") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let path_str = path.unwrap_or("");

    // Get directory listing
    match db.list_directory(path_str) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            let music_dir = state.music_dir.as_deref();

            // Songs first, then directories (matches MPD's lsinfo output order)
            for song in &listing.songs {
                let display_path = strip_music_dir_prefix(song.path.as_str(), music_dir);
                let mut display_song = song.clone();
                display_song.path = display_path.into();
                resp.song(&display_song, None, None);
            }
            for (dir, mtime) in &listing.directories {
                let display_dir = strip_music_dir_prefix(dir, music_dir);
                resp.field("directory", display_dir);
                if *mtime > 0 {
                    let ts = format_iso8601_timestamp(*mtime);
                    resp.field("Last-Modified", &ts);
                }
            }

            // For root directory, also list playlists
            if (path_str.is_empty() || path_str == "/")
                && let Ok(playlists) = db.list_playlists()
            {
                for playlist in &playlists {
                    resp.field("playlist", &playlist.name);
                    let timestamp_str = format_iso8601_timestamp(playlist.last_modified);
                    resp.field("Last-Modified", &timestamp_str);
                }
            }

            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "lsinfo", &format!("Error: {e}")),
    }
}

pub async fn handle_listall_command(state: &AppState, path: Option<&str>) -> String {
    let db = match open_db(state, "listall") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let path_str = path.unwrap_or("");
    let mut resp = ResponseBuilder::new();

    let result = db.walk_recursive(path_str, &mut |entry| {
        match entry {
            rmpd_library::WalkEntry::Song(song) => {
                resp.field("file", &song.path);
            }
            rmpd_library::WalkEntry::Directory(dir) => {
                resp.field("directory", dir);
            }
        }
        Ok(())
    });

    match result {
        Ok(()) => resp.ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listall", &format!("Error: {e}")),
    }
}

pub async fn handle_listallinfo_command(state: &AppState, path: Option<&str>) -> String {
    let db = match open_db(state, "listallinfo") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let path_str = path.unwrap_or("");
    let mut resp = ResponseBuilder::new();

    let result = db.walk_recursive(path_str, &mut |entry| {
        match entry {
            rmpd_library::WalkEntry::Song(song) => {
                resp.song(song, None, None);
            }
            rmpd_library::WalkEntry::Directory(dir) => {
                resp.field("directory", dir);
            }
        }
        Ok(())
    });

    match result {
        Ok(()) => resp.ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listallinfo", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_searchadd_command(state: &AppState, tag: &str, value: &str) -> String {
    let db = match open_db(state, "searchadd") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Search for songs
    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.search_songs(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "searchadd",
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
                    "searchadd",
                    &format!("query error: {e}"),
                );
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
    let db = match open_db(state, "findadd") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // findadd uses exact match (unlike searchadd which uses partial/FTS for "any")
    let songs = if tag.eq_ignore_ascii_case("any") {
        match db.find_songs_any(value) {
            Ok(s) => s,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYSTEM,
                    0,
                    "findadd",
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
                    "findadd",
                    &format!("query error: {e}"),
                );
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

pub async fn handle_listfiles_command(state: &AppState, uri: Option<&str>) -> String {
    let path = uri.unwrap_or("");
    // Prefer filesystem listing (like MPD) to show all files with size.
    if let Some(music_dir) = state.music_dir.as_deref() {
        let full_path = if path.is_empty() {
            std::path::PathBuf::from(music_dir)
        } else {
            std::path::PathBuf::from(music_dir).join(path)
        };

        // Safety: reject path traversal
        if path.contains("..") {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "listfiles", "bad path");
        }

        match std::fs::read_dir(&full_path) {
            Ok(entries) => {
                let mut resp = ResponseBuilder::new();
                // MPD streams entries in readdir order with dirs and files
                // interleaved — no sorting, no separation.
                for entry in entries.flatten() {
                    let name = match entry.file_name().into_string() {
                        Ok(n) => n,
                        Err(_) => continue, // skip non-UTF8 names
                    };
                    // Skip hidden files and special entries (MPD skips . and ..)
                    if name.starts_with('.') {
                        continue;
                    }
                    // Skip names containing newlines (MPD does this)
                    if name.contains('\n') {
                        continue;
                    }
                    let meta = match entry.metadata() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    if meta.is_file() {
                        resp.field("file", &name);
                        resp.field("size", meta.len());
                    } else if meta.is_dir() {
                        resp.field("directory", &name);
                    } else {
                        continue;
                    }

                    if let Ok(mtime) = meta.modified() {
                        let ts = format_iso8601_timestamp(
                            mtime
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64,
                        );
                        resp.field("Last-Modified", &ts);
                    }
                }
                return resp.ok();
            }
            Err(_) => {
                // Fall through to DB-based listing
            }
        }
    }

    // Fallback: use database listing when music_dir is not available
    let db = match open_db(state, "listfiles") {
        Ok(d) => d,
        Err(e) => return e,
    };
    match db.list_directory(path) {
        Ok(listing) => {
            let mut resp = ResponseBuilder::new();
            let music_dir = state.music_dir.as_deref();
            // MPD emits directories before files in listfiles
            for (dir, mtime) in &listing.directories {
                let display_dir = strip_music_dir_prefix(dir, music_dir);
                let basename = display_dir.rsplit('/').next().unwrap_or(display_dir);
                resp.field("directory", basename);
                if *mtime > 0 {
                    let ts = format_iso8601_timestamp(*mtime);
                    resp.field("Last-Modified", &ts);
                }
            }
            for song in &listing.songs {
                let display_path = strip_music_dir_prefix(song.path.as_str(), music_dir);
                let filename = display_path.rsplit('/').next().unwrap_or(display_path);
                resp.field("file", filename);
                if song.last_modified > 0 {
                    let ts = format_iso8601_timestamp(song.last_modified);
                    resp.field("Last-Modified", &ts);
                }
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "listfiles", &format!("Error: {e}")),
    }
}

/// Count search results with optional grouping
///
/// This is a convenience wrapper for count_command
pub async fn handle_searchcount_command(
    state: &AppState,
    tag: &str,
    value: &str,
    group: Option<&str>,
) -> String {
    let db = match open_db(state, "searchcount") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // searchcount does case-insensitive substring matching (like `search`, not `count`)
    let songs = match db.search_songs_by_tag(tag, value) {
        Ok(s) => s,
        Err(e) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM, 0, "searchcount",
                &format!("query error: {e}"),
            );
        }
    };

    let mut resp = ResponseBuilder::new();

    if let Some(group_tag) = group {
        use std::collections::HashMap;
        let mut groups: HashMap<String, (usize, u64)> = HashMap::new();
        for song in &songs {
            let vals = song.tag_values_with_fallback(group_tag);
            let vals: Vec<&str> = if vals.is_empty() { vec![""] } else { vals };
            for group_value in vals {
                let entry = groups.entry(group_value.to_string()).or_insert((0, 0));
                entry.0 += 1;
                if let Some(duration) = song.duration {
                    entry.1 += duration.as_secs();
                }
            }
        }
        let mut sorted: Vec<_> = groups.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let tag_key = rmpd_core::song::canonical_tag_name(&group_tag.to_lowercase());
        for (val, (count, playtime)) in &sorted {
            resp.field(tag_key, val);
            resp.field("songs", count);
            resp.field("playtime", playtime);
        }
    } else {
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

/// Generate chromaprint fingerprint for audio file
///
/// IMPLEMENTATION NOTE:
/// Chromaprint support requires:
/// 1. chromaprint-sys-next crate (Rust bindings to libchromaprint)
/// 2. System libchromaprint library installed (apt-get install libchromaprint-dev)
/// 3. Audio decoding to PCM samples (integrate with decoder.rs)
/// 4. Generate fingerprint from PCM data
/// 5. Return base64-encoded fingerprint string
///
/// This is a stub implementation that validates the file exists but
/// returns "not available" until full chromaprint integration is added.
pub async fn handle_getfingerprint_command(state: &AppState, uri: &str) -> String {
    // Resolve the file path
    let file_path = if uri.starts_with('/') {
        uri.to_string()
    } else {
        match &state.music_dir {
            Some(music_dir) => format!("{music_dir}/{uri}"),
            None => uri.to_string(),
        }
    };

    // Check if file exists
    if !std::path::Path::new(&file_path).exists() {
        return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "getfingerprint", "No such file");
    }

    // Chromaprint library not yet integrated
    ResponseBuilder::error(
        ACK_ERROR_SYSTEM,
        0,
        "getfingerprint",
        "chromaprint not available",
    )
}

/// Read file metadata comments
///
/// Returns comment field from the song metadata
pub async fn handle_readcomments_command(state: &AppState, uri: &str) -> String {
    let db = match open_db(state, "readcomments") {
        Ok(d) => d,
        Err(e) => return e,
    };

    if let Ok(Some(song)) = db.get_song_by_path(uri) {
        let mut resp = ResponseBuilder::new();
        // readcomments emits all raw metadata fields, uppercase, in file order.
        for (tag, value) in &song.tags {
            if !value.is_empty() {
                let key = tag.to_uppercase();
                resp.field(&key, value);
            }
        }
        return resp.ok();
    }
    ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "readcomments", "No such file")
}
