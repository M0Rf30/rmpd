//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_EXIST, ACK_ERROR_NO_EXIST, ACK_ERROR_SYS, format_iso8601_timestamp,
    open_db,
};
use std::path::Path;

fn strip_file_uri_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("file://localhost") {
        rest.to_string()
    } else if let Some(rest) = value.strip_prefix("file:///") {
        format!("/{rest}")
    } else if let Some(rest) = value.strip_prefix("file://") {
        rest.to_string()
    } else {
        value.to_string()
    }
}

/// Notify idle clients that the set or contents of stored playlists changed,
/// mirroring MPD's `idle_add(IDLE_STORED_PLAYLIST)` after a successful mutation.
fn notify_stored_playlist(state: &AppState) {
    state
        .event_bus
        .emit(rmpd_core::event::Event::StoredPlaylistChanged);
}

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

fn read_pls_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(playlist_dir).join(format!("{name}.pls"));
    let content = std::fs::read_to_string(&path).map_err(|_| "No such playlist".to_string())?;
    let mut paths = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once('=')
            && key.trim().len() >= 4
            && key.trim()[..4].eq_ignore_ascii_case("file")
        {
            paths.push(strip_file_uri_prefix(value.trim()));
        }
    }

    Ok(paths)
}

fn extract_xml_tag_content(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut results = Vec::new();
    let mut remaining = xml;
    while let Some(start) = remaining.find(&open) {
        let after_open = &remaining[start + open.len()..];
        if let Some(end) = after_open.find(&close) {
            let content = after_open[..end].trim().to_string();
            results.push(content);
            remaining = &after_open[end + close.len()..];
        } else {
            break;
        }
    }
    results
}

fn read_xspf_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(playlist_dir).join(format!("{name}.xspf"));
    let content = std::fs::read_to_string(&path).map_err(|_| "No such playlist".to_string())?;

    let mut paths = extract_xml_tag_content(&content, "location");
    if paths.is_empty() {
        paths = extract_xml_tag_content(&content, "file");
    }

    Ok(paths
        .into_iter()
        .map(|p| strip_file_uri_prefix(p.trim()))
        .collect())
}

fn read_asx_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(playlist_dir).join(format!("{name}.asx"));
    let content = std::fs::read_to_string(&path).map_err(|_| "No such playlist".to_string())?;

    // ASX: <REF HREF="..."/> or <ref href="..."/>
    let mut paths = Vec::new();
    let mut remaining = content.as_str();
    while let Some(pos) = remaining.to_ascii_lowercase().find("<ref ") {
        let chunk = &remaining[pos..];
        if let Some(href_pos) = chunk.to_ascii_lowercase().find("href=") {
            let after_href = &chunk[href_pos + 5..];
            let trimmed = after_href.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let (quote, rest) = if let Some(s) = trimmed.strip_prefix('"') {
                ('"', s)
            } else if let Some(s) = trimmed.strip_prefix('\'') {
                ('\'', s)
            } else {
                remaining = &remaining[pos + 5..];
                continue;
            };
            if let Some(end) = rest.find(quote) {
                paths.push(strip_file_uri_prefix(&rest[..end]));
            }
        }
        remaining = &remaining[pos + 5..];
    }
    Ok(paths)
}

fn read_cue_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let cue_path = std::path::Path::new(playlist_dir).join(format!("{name}.cue"));
    let content = std::fs::read_to_string(&cue_path).map_err(|_| "No such playlist".to_string())?;
    let mut paths = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.len() < 4 || !trimmed[..4].eq_ignore_ascii_case("file") {
            continue;
        }

        if let Some(start_quote) = trimmed.find('"') {
            let rest = &trimmed[start_quote + 1..];
            if let Some(end_quote) = rest.find('"') {
                let file_ref = &rest[..end_quote];
                let file_path = std::path::Path::new(file_ref);
                let resolved = if file_path.is_absolute() {
                    file_path.to_path_buf()
                } else {
                    std::path::Path::new(playlist_dir).join(file_path)
                };
                let resolved_str = resolved.to_string_lossy().to_string();
                if !paths.contains(&resolved_str) {
                    paths.push(resolved_str);
                }
            }
        }
    }

    Ok(paths)
}

/// Parse a `.cue` sheet into virtual-track songs paired with playback ranges.
/// Each track becomes a `Song` whose `path` is the referenced audio file plus
/// CUE-derived tags (title/artist/album/albumartist/track), paired with its
/// `(start, end)` range in seconds. A file's last track uses `end == start`
/// to mean "play to the end of the file".
fn read_cue_tracks(
    playlist_dir: &str,
    name: &str,
) -> Result<Vec<(rmpd_core::song::Song, (f64, f64))>, String> {
    use std::borrow::Cow;
    let cue_path = Path::new(playlist_dir).join(format!("{name}.cue"));
    let content = std::fs::read_to_string(&cue_path).map_err(|_| "No such playlist".to_string())?;
    let mut out = Vec::new();
    for t in rmpd_library::parse_cue(&content) {
        let file_path = Path::new(&t.file);
        let resolved = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            Path::new(playlist_dir).join(file_path)
        };
        let mut tags: Vec<(Cow<'static, str>, String)> = Vec::new();
        if let Some(v) = t.title {
            tags.push((Cow::Borrowed("title"), v));
        }
        if let Some(v) = t.performer {
            tags.push((Cow::Borrowed("artist"), v));
        }
        if let Some(v) = t.album {
            tags.push((Cow::Borrowed("album"), v));
        }
        if let Some(v) = t.album_performer {
            tags.push((Cow::Borrowed("albumartist"), v));
        }
        tags.push((Cow::Borrowed("track"), t.number.to_string()));
        let duration = t
            .end
            .map(|e| std::time::Duration::from_secs_f64((e - t.start).max(0.0)));
        let song = rmpd_core::song::Song {
            id: 0,
            path: camino::Utf8PathBuf::from(resolved.to_string_lossy().to_string()),
            duration,
            sample_rate: None,
            channels: None,
            bits_per_sample: None,
            bitrate: None,
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
            tags,
        };
        out.push((song, (t.start, t.end.unwrap_or(t.start))));
    }
    Ok(out)
}

fn read_playlist(playlist_dir: &str, name: &str) -> Result<Vec<String>, String> {
    let path_m3u = std::path::Path::new(playlist_dir).join(format!("{name}.m3u"));
    let path_pls = std::path::Path::new(playlist_dir).join(format!("{name}.pls"));
    let path_xspf = std::path::Path::new(playlist_dir).join(format!("{name}.xspf"));
    let path_cue = std::path::Path::new(playlist_dir).join(format!("{name}.cue"));

    let path_asx = std::path::Path::new(playlist_dir).join(format!("{name}.asx"));

    if path_m3u.exists() {
        read_m3u_playlist(playlist_dir, name)
    } else if path_pls.exists() {
        read_pls_playlist(playlist_dir, name)
    } else if path_xspf.exists() {
        read_xspf_playlist(playlist_dir, name)
    } else if path_cue.exists() {
        read_cue_playlist(playlist_dir, name)
    } else if path_asx.exists() {
        read_asx_playlist(playlist_dir, name)
    } else {
        Err(format!("No such playlist: {name}"))
    }
}

pub async fn handle_listplaylists_command(state: &AppState) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "listplaylists",
                "playlist directory not configured",
            );
        }
    };

    match tokio::task::spawn_blocking(move || {
        let mut resp = ResponseBuilder::new();

        // Read playlist files from playlist directory, matching MPD's filesystem-based approach
        let dir = match std::fs::read_dir(&playlist_dir) {
            Ok(d) => d,
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "listplaylists",
                    &format!("Error reading playlist directory: {e}"),
                );
            }
        };

        let mut entries: Vec<(String, i64)> = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if matches!(
                ext,
                Some("m3u") | Some("pls") | Some("xspf") | Some("cue") | Some("asx")
            ) && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
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
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylists", "internal error"),
    }
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
                ACK_ERROR_SYS,
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
                return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "save", "No such playlist");
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

    let name_owned = name.to_string();
    let result = tokio::task::spawn_blocking(move || {
        // For append mode, prepend existing paths
        let paths_to_write: Vec<String> = if matches!(mode, SaveMode::Append) {
            let mut existing = read_m3u_playlist(&playlist_dir, &name_owned).unwrap_or_default();
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
        std::fs::write(&pl_path, &content)
    })
    .await;

    match result {
        Ok(Ok(_)) => {
            notify_stored_playlist(state);
            ResponseBuilder::new().ok()
        }
        Ok(Err(e)) => ResponseBuilder::error(
            ACK_ERROR_SYS,
            0,
            "save",
            &format!("Error writing playlist: {e}"),
        ),
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "save", "internal error"),
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
                ACK_ERROR_SYS,
                0,
                "load",
                "playlist directory not configured",
            );
        }
    };

    // A `.cue` sheet (when no higher-priority playlist of the same name exists)
    // expands into virtual tracks with playback ranges instead of plain paths.
    let cue_only = {
        let p = |ext: &str| Path::new(&playlist_dir).join(format!("{name}.{ext}"));
        p("cue").exists()
            && !p("m3u").exists()
            && !p("pls").exists()
            && !p("xspf").exists()
            && !p("asx").exists()
    };
    if cue_only {
        return load_cue_virtual_tracks(state, &playlist_dir, name, range, position).await;
    }

    let state_clone = state.clone();
    let playlist_dir_clone = playlist_dir.clone();
    let name_owned = name.to_string();
    let songs = match tokio::task::spawn_blocking(move || {
        let mut paths = read_playlist(&playlist_dir_clone, &name_owned)
            .map_err(|e| ResponseBuilder::error(ACK_ERROR_SYS, 0, "load", &e))?;

        // Apply range filter if specified
        if let Some((start, end)) = range {
            let start = start as usize;
            let end = (end as usize).min(paths.len());
            if start <= paths.len() {
                paths = paths[start..end].to_vec();
            } else {
                return Err(ResponseBuilder::error(ACK_ERROR_ARG, 0, "load", "Invalid range"));
            }
        }

        // Look up songs from DB; fall back to stub Song if not found
        let db = open_db(&state_clone, "load")?;
        let songs: Vec<rmpd_core::song::Song> = paths
            .iter()
            .filter_map(|path| db.get_song_by_path(path).ok().flatten())
            .collect();
        Ok(songs)
    })
    .await
    {
        Ok(Ok(songs)) => songs,
        Ok(Err(e)) => return e,
        Err(_) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "load", "internal error"),
    };

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

    crate::helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

/// Load a `.cue` sheet as virtual tracks: each track is added to the queue with
/// its own playback range (start/end in seconds) so playback is restricted to
/// that segment of the underlying audio file.
async fn load_cue_virtual_tracks(
    state: &AppState,
    playlist_dir: &str,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    let mut tracks = match read_cue_tracks(playlist_dir, name) {
        Ok(t) => t,
        Err(e) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "load", &e),
    };

    if let Some((start, end)) = range {
        let start = start as usize;
        let end = (end as usize).min(tracks.len());
        if start > tracks.len() || start > end {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "load", "Invalid range");
        }
        tracks = tracks[start..end].to_vec();
    }

    {
        let mut queue = state.queue.write().await;
        for (i, (song, song_range)) in tracks.into_iter().enumerate() {
            let pos = position.map(|p| p + i as u32);
            let id = queue.add_at(song, pos);
            queue.set_range_by_id(id, Some(song_range));
        }
    }

    crate::helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

pub async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    let state = state.clone();
    let name = name.to_string();
    let tag = tag.to_string();
    let value = value.to_string();
    match tokio::task::spawn_blocking(move || {
        let playlist_dir = match &state.playlist_dir {
            Some(d) => d.clone(),
            None => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "searchaddpl",
                    "playlist directory not configured",
                );
            }
        };
        let db = match open_db(&state, "searchaddpl") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let songs = if tag.eq_ignore_ascii_case("any") {
            match db.search_songs(&value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        "searchaddpl",
                        &format!("search error: {e}"),
                    );
                }
            }
        } else {
            match db.find_songs(&tag, &value) {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        "searchaddpl",
                        &format!("query error: {e}"),
                    );
                }
            }
        };

        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        let mut paths = if pl_path.exists() {
            read_m3u_playlist(&playlist_dir, &name).unwrap_or_default()
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
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "searchaddpl", &format!("Error: {e}"))
            }
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "searchaddpl", "internal error"),
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
                ACK_ERROR_SYS,
                0,
                "listplaylist",
                "playlist directory not configured",
            );
        }
    };
    let name = name.to_string();

    match tokio::task::spawn_blocking(move || {
        let paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(e) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylist", &e),
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
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylist", "internal error"),
    }
}
pub async fn handle_listplaylistinfo_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
) -> String {
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let playlist_dir = match &state.playlist_dir {
            Some(d) => d.clone(),
            None => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "listplaylistinfo",
                    "playlist directory not configured",
                );
            }
        };

        let paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(e) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylistinfo", &e),
        };
        let db = match open_db(&state, "listplaylistinfo") {
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
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylistinfo", "internal error"),
    }
}

pub async fn handle_playlistadd_command(
    state: &AppState,
    name: &str,
    uri: &str,
    position: Option<u32>,
) -> String {
    let state = state.clone();
    let name = name.to_string();
    let uri = uri.to_string();
    match tokio::task::spawn_blocking(move || {
        let playlist_dir = match &state.playlist_dir {
            Some(d) => d.clone(),
            None => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "playlistadd",
                    "playlist directory not configured",
                );
            }
        };
        let db = match open_db(&state, "playlistadd") {
            Ok(d) => d,
            Err(e) => return e,
        };

        // Look up songs matching the URI in the database (song or directory prefix)
        let songs = match db.find_songs_by_prefix(&uri) {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "playlistadd",
                    "No such directory",
                );
            }
            Err(e) => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "playlistadd",
                    &format!("Error: {e}"),
                );
            }
        };

        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        let mut paths = if pl_path.exists() {
            read_m3u_playlist(&playlist_dir, &name).unwrap_or_default()
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
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistadd", &format!("Error: {e}"))
            }
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistadd", "internal error"),
    }
}

pub async fn handle_playlistclear_command(state: &AppState, name: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "playlistclear",
                "playlist directory not configured",
            );
        }
    };
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        if !pl_path.exists() {
            return ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "playlistclear",
                "No such playlist",
            );
        }
        match std::fs::write(&pl_path, "") {
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistclear", &format!("Error: {e}"))
            }
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistclear", "internal error"),
    }
}

pub async fn handle_playlistdelete_command(state: &AppState, name: &str, position: u32) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "playlistdelete",
                "playlist directory not configured",
            );
        }
    };
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let mut paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(e) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistdelete", &e),
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
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistdelete", &format!("Error: {e}"))
            }
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistdelete", "internal error"),
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
                ACK_ERROR_SYS,
                0,
                "playlistmove",
                "playlist directory not configured",
            );
        }
    };
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let mut paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(e) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistmove", &e),
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
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistmove", &format!("Error: {e}"))
            }
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistmove", "internal error"),
    }
}

pub async fn handle_rm_command(state: &AppState, name: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "rm",
                "playlist directory not configured",
            );
        }
    };
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        if !pl_path.exists() {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "rm", "No such playlist");
        }
        match std::fs::remove_file(&pl_path) {
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "rm", &format!("Error: {e}")),
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "rm", "internal error"),
    }
}

pub async fn handle_rename_command(state: &AppState, from: &str, to: &str) -> String {
    let playlist_dir = match &state.playlist_dir {
        Some(d) => d.clone(),
        None => {
            return ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "rename",
                "playlist directory not configured",
            );
        }
    };
    let state = state.clone();
    let from = from.to_string();
    let to = to.to_string();
    match tokio::task::spawn_blocking(move || {
        let from_path = Path::new(&playlist_dir).join(format!("{from}.m3u"));
        let to_path = Path::new(&playlist_dir).join(format!("{to}.m3u"));
        if !from_path.exists() {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "rename", "No such playlist");
        }
        if to_path.exists() {
            return ResponseBuilder::error(ACK_ERROR_EXIST, 0, "rename", "Playlist exists already");
        }
        match std::fs::rename(&from_path, &to_path) {
            Ok(_) => {
                notify_stored_playlist(&state);
                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "rename", &format!("Error: {e}")),
        }
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "rename", "internal error"),
    }
}

// Stored playlist search and utility commands
pub async fn handle_searchplaylist_command(
    state: &AppState,
    name: &str,
    tag: &str,
    value: &str,
) -> String {
    let state = state.clone();
    let name = name.to_string();
    let tag = tag.to_string();
    let value = value.to_string();
    match tokio::task::spawn_blocking(move || {
        let playlist_dir = match &state.playlist_dir {
            Some(d) => d.clone(),
            None => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "searchplaylist",
                    "playlist directory not configured",
                );
            }
        };
        let paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "searchplaylist",
                    "No such playlist",
                );
            }
        };
        let db = match open_db(&state, "searchplaylist") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let mut resp = ResponseBuilder::new();
        let value_lower = value.to_lowercase();
        let tag_lower = tag.to_lowercase();
        for path in &paths {
            if let Ok(Some(song)) = db.get_song_by_path(path)
                && song.tag_contains(&tag_lower, &value_lower)
            {
                resp.song(&song, None, None);
            }
        }
        resp.ok()
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "searchplaylist", "internal error"),
    }
}

pub async fn handle_playlistlength_command(state: &AppState, name: &str) -> String {
    let state = state.clone();
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || {
        let playlist_dir = match &state.playlist_dir {
            Some(d) => d.clone(),
            None => {
                return ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "playlistlength",
                    "playlist directory not configured",
                );
            }
        };
        let paths = match read_m3u_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "playlistlength",
                    "No such playlist",
                );
            }
        };
        let db = match open_db(&state, "playlistlength") {
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
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "playlistlength", "internal error"),
    }
}

