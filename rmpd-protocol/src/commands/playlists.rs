//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_EXIST, ACK_ERROR_SYSTEM, format_iso8601_timestamp, open_db,
    song_tag_contains,
};
use std::path::Path;

/// Parse an .m3u playlist file and return the list of relative paths.
/// Lines starting with '#' are comments and are skipped.
fn read_m3u_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(playlist_dir).join(format!("{name}.m3u"));
    let content = std::fs::read_to_string(&path).map_err(|_| "No such playlist".to_string())?;
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
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "listplaylists",
                "playlist directory not configured",
            );
        }
    };

    let mut resp = ResponseBuilder::new();

    // Read .m3u files from playlist directory, matching MPD's filesystem-based approach
    let dir = match std::fs::read_dir(&playlist_dir) {
        Ok(d) => d,
        Err(e) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "listplaylists",
                &format!("Error reading playlist directory: {e}"),
            );
        }
    };

    let mut entries: Vec<(String, i64)> = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("m3u")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push((stem.to_string(), mtime));
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

    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "save",
                "playlist directory not configured",
            );
        }
    };
    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    let mode = mode.unwrap_or(SaveMode::Replace);

    // Enforce mode preconditions (matching MPD's PlaylistSave.cxx spl_save_queue)
    match mode {
        SaveMode::Create => {
            if pl_path.exists() {
                return ResponseBuilder::error(
                    ACK_ERROR_EXIST,
                    0,
                    "save",
                    "Playlist already exists",
                );
            }
        }
        SaveMode::Append => {
            if !pl_path.exists() {
                return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "save", "No such playlist");
            }
        }
        SaveMode::Replace => {
            // Replace works whether playlist exists or not (create-or-overwrite)
        }
    }

    // Collect current queue paths
    let new_paths: Vec<String> = {
        let queue = state.queue.read().await;
        queue
            .items()
            .iter()
            .map(|item| item.song.path.to_string())
            .collect()
    };

    // For append mode, prepend existing paths
    let paths_to_write: Vec<String> = if matches!(mode, SaveMode::Append) {
        let mut existing = read_m3u_playlist(&playlist_dir, name).unwrap_or_default();
        existing.extend(new_paths);
        existing
    } else {
        new_paths
    };

    // Write the .m3u file
    let content = paths_to_write
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        content
    } else {
        content + "\n"
    };
    match std::fs::write(&pl_path, &content) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(
            ACK_ERROR_SYSTEM,
            0,
            "save",
            &format!("Error writing playlist: {e}"),
        ),
    }
}

pub async fn handle_load_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "load",
                "playlist directory not configured",
            );
        }
    };

    let mut paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "load", &e),
    };

    // Apply range filter if specified
    if let Some((start, end)) = range {
        let start = start as usize;
        let end = (end as usize).min(paths.len());
        if start <= paths.len() {
            paths = paths[start..end].to_vec();
        } else {
            return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "load", "Invalid range");
        }
    }

    // Look up songs from DB; fall back to stub Song if not found
    let db = match open_db(state, "load") {
        Ok(d) => d,
        Err(e) => return e,
    };
    let songs: Vec<rmpd_core::song::Song> = paths
        .iter()
        .filter_map(|path| db.get_song_by_path(path).ok().flatten())
        .collect();

    {
        let mut queue = state.queue.write().await;
        if let Some(pos) = position {
            for (i, song) in songs.into_iter().enumerate() {
                queue.add_at(song, Some(pos + i as u32));
            }
        } else {
            for song in songs {
                queue.add(song);
            }
        }
    }

    let mut status = state.status.write().await;
    status.playlist_version += 1;
    status.playlist_length = state.queue.read().await.len() as u32;
    ResponseBuilder::new().ok()
}

pub async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "searchaddpl",
                "playlist directory not configured",
            );
        }
    };
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

    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    let mut paths = if pl_path.exists() {
        read_m3u_playlist(&playlist_dir, name).unwrap_or_default()
    } else {
        vec![]
    };
    for song in &songs {
        paths.push(song.path.to_string());
    }
    let content = paths
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        content
    } else {
        content + "\n"
    };
    match std::fs::write(&pl_path, &content) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "searchaddpl", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_listplaylist_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "listplaylist",
                "playlist directory not configured",
            );
        }
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
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "listplaylistinfo",
                "playlist directory not configured",
            );
        }
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
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistadd",
                "playlist directory not configured",
            );
        }
    };
    let db = match open_db(state, "playlistadd") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Look up songs matching the URI in the database (song or directory prefix)
    let songs = match db.find_songs_by_prefix(uri) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistadd", "No such directory");
        }
        Err(e) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistadd",
                &format!("Error: {e}"),
            );
        }
    };

    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    let mut paths = if pl_path.exists() {
        read_m3u_playlist(&playlist_dir, name).unwrap_or_default()
    } else {
        vec![]
    };

    // Collect new paths to append (sorted, matching MPD behavior)
    let start_pos = position.map(|p| p as usize);
    let new_paths: Vec<String> = songs.iter().map(|s| s.path.to_string()).collect();
    if let Some(pos) = start_pos {
        let pos = pos.min(paths.len());
        for (i, p) in new_paths.into_iter().enumerate() {
            paths.insert(pos + i, p);
        }
    } else {
        paths.extend(new_paths);
    }

    let content = paths
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        content
    } else {
        content + "\n"
    };
    match std::fs::write(&pl_path, &content) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistadd", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_playlistclear_command(state: &AppState, name: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistclear",
                "playlist directory not configured",
            );
        }
    };
    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    if !pl_path.exists() {
        return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistclear", "No such playlist");
    }
    match std::fs::write(&pl_path, "") {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistclear", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_playlistdelete_command(state: &AppState, name: &str, position: u32) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistdelete",
                "playlist directory not configured",
            );
        }
    };
    let mut paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistdelete", &e),
    };
    let pos = position as usize;
    if pos >= paths.len() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistdelete", "Bad song index");
    }
    paths.remove(pos);
    let content = paths
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        content
    } else {
        content + "\n"
    };
    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    match std::fs::write(&pl_path, &content) {
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
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistmove",
                "playlist directory not configured",
            );
        }
    };
    let mut paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistmove", &e),
    };
    let from = from as usize;
    let to = to as usize;
    if from >= paths.len() || to > paths.len() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistmove", "Bad song index");
    }
    let song = paths.remove(from);
    let insert_pos = if to > from { to - 1 } else { to };
    paths.insert(insert_pos.min(paths.len()), song);
    let content = paths
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let content = if content.is_empty() {
        content
    } else {
        content + "\n"
    };
    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    match std::fs::write(&pl_path, &content) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => {
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "playlistmove", &format!("Error: {e}"))
        }
    }
}

pub async fn handle_rm_command(state: &AppState, name: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "rm",
                "playlist directory not configured",
            );
        }
    };
    let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
    if !pl_path.exists() {
        return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "rm", "No such playlist");
    }
    match std::fs::remove_file(&pl_path) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "rm", &format!("Error: {e}")),
    }
}

pub async fn handle_rename_command(state: &AppState, from: &str, to: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "rename",
                "playlist directory not configured",
            );
        }
    };
    let from_path = Path::new(&playlist_dir).join(format!("{from}.m3u"));
    let to_path = Path::new(&playlist_dir).join(format!("{to}.m3u"));
    if !from_path.exists() {
        return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "rename", "No such playlist");
    }
    if to_path.exists() {
        return ResponseBuilder::error(ACK_ERROR_EXIST, 0, "rename", "Playlist exists already");
    }
    match std::fs::rename(&from_path, &to_path) {
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
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "searchplaylist",
                "playlist directory not configured",
            );
        }
    };
    let paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(_) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "searchplaylist",
                "No such playlist",
            );
        }
    };
    let db = match open_db(state, "searchplaylist") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let mut resp = ResponseBuilder::new();
    let value_lower = value.to_lowercase();
    let tag_lower = tag.to_lowercase();
    for path in &paths {
        if let Ok(Some(song)) = db.get_song_by_path(path)
            && song_tag_contains(&song, &tag_lower, &value_lower)
        {
            resp.song(&song, None, None);
        }
    }
    resp.ok()
}

pub async fn handle_playlistlength_command(state: &AppState, name: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistlength",
                "playlist directory not configured",
            );
        }
    };
    let paths = match read_m3u_playlist(&playlist_dir, name) {
        Ok(p) => p,
        Err(_) => {
            return ResponseBuilder::error(
                ACK_ERROR_SYSTEM,
                0,
                "playlistlength",
                "No such playlist",
            );
        }
    };
    let db = match open_db(state, "playlistlength") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let mut total_duration = 0.0_f64;
    let mut count = 0usize;
    for path in &paths {
        if let Ok(Some(song)) = db.get_song_by_path(path) {
            total_duration += song.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);
            count += 1;
        } else {
            count += 1; // count even if not in DB
        }
    }

    let mut resp = ResponseBuilder::new();
    resp.field("songs", count.to_string());
    resp.field("playtime", format!("{total_duration:.3}"));
    resp.ok()
}
