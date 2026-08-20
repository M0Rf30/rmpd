//! Stored playlist management command handlers

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_EXIST, ACK_ERROR_NO_EXIST, ACK_ERROR_PLAYER_SYNC, ACK_ERROR_SYS,
    apply_range, format_iso8601_timestamp, open_db, parse_sort_tag, sort_songs,
};
use crate::parser::InsertPosition;
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

/// Reject playlist names that could escape `playlist_directory` or otherwise
/// misbehave. Mirrors MPD's `spl_valid_name()` (PlaylistFile.cxx) exactly:
/// on a non-Windows build the only forbidden character is '/' (the
/// directory separator), plus the name must be non-empty. Names like "."
/// or ".." are valid single path components once the ".m3u" suffix is
/// appended, so MPD does not special-case them.
pub(crate) fn validate_playlist_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') {
        return Err("Bad playlist name".to_string());
    }
    Ok(())
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
            // MPD's `listplaylists` enumerates only the stored-playlist
            // directory's `.m3u` files (`ListPlaylistFiles()` in
            // PlaylistFile.cxx filters on `PLAYLIST_FILE_SUFFIX`); playlist
            // plugin files (.pls/.xspf/.cue/.asx) are readable via
            // `listplaylist`/`load` but are not "stored playlists".
            if ext == Some("m3u")
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
    })
    .await
    {
        Ok(resp) => resp,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listplaylists", "internal error"),
    }
}

pub async fn handle_save_command(state: &AppState, name: &str, mode: Option<String>) -> String {
    use crate::parser::SaveMode;

    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "save", &e);
    }

    // Mode names are matched case-sensitively, as MPD does with
    // `StringIsEqual` in `handle_save` (PlaylistCommands.cxx).
    let mode = match mode.as_deref() {
        None => SaveMode::Create,
        Some("create") => SaveMode::Create,
        Some("append") => SaveMode::Append,
        Some("replace") => SaveMode::Replace,
        Some(_) => {
            return ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                "save",
                "Unrecognized save mode, expected one of 'create', 'append', 'replace'",
            );
        }
    };

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

    // Enforce mode preconditions (matching MPD's PlaylistSave.cxx
    // spl_save_queue: CREATE fails if the playlist exists; APPEND and
    // REPLACE both fail unless it already exists — "replace" is not a
    // create-or-overwrite despite the name).
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
        SaveMode::Append | SaveMode::Replace => {
            if !pl_path.exists() {
                return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "save", "No such playlist");
            }
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

/// Resolve `load`'s POSITION argument to an absolute queue index, mirroring
/// MPD's `ParseInsertPosition()` (PositionArg.cxx): `+N`/`-N` are relative to
/// the currently playing song, bounds-checked against the queue's length and
/// current position *before* the load inserts anything.
fn resolve_insert_position(
    pos: InsertPosition,
    queue_len: u32,
    current_song_position: Option<u32>,
) -> Result<u32, String> {
    match pos {
        InsertPosition::Absolute(n) => {
            if n > queue_len {
                Err("Bad song index".to_string())
            } else {
                Ok(n)
            }
        }
        InsertPosition::After(n) => {
            let current = current_song_position.ok_or_else(|| "No current song".to_string())?;
            let max = queue_len - current - 1;
            if n > max {
                Err("Bad song index".to_string())
            } else {
                Ok(current + 1 + n)
            }
        }
        InsertPosition::Before(n) => {
            let current = current_song_position.ok_or_else(|| "No current song".to_string())?;
            if n > current {
                Err("Bad song index".to_string())
            } else {
                Ok(current - n)
            }
        }
    }
}

pub async fn handle_load_command(
    state: &AppState,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<InsertPosition>,
) -> String {
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "load", &e);
    }

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

    // Resolve a relative (+N/-N) or absolute POSITION against the queue's
    // pre-load length and current song, mirroring MPD's ParseInsertPosition
    // call (which runs before the songs are loaded into the queue).
    let resolved_position = match position {
        None => None,
        Some(pos) => {
            let queue_len = state.queue.read().await.len() as u32;
            let current = state.status.read().await.current_song.map(|q| q.position);
            match resolve_insert_position(pos, queue_len, current) {
                Ok(p) => Some(p),
                Err(e) => {
                    let code = if e == "No current song" {
                        ACK_ERROR_PLAYER_SYNC
                    } else {
                        ACK_ERROR_ARG
                    };
                    return ResponseBuilder::error(code, 0, "load", &e);
                }
            }
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
        return load_cue_virtual_tracks(state, &playlist_dir, name, range, resolved_position).await;
    }

    let state_clone = state.clone();
    let playlist_dir_clone = playlist_dir.clone();
    let name_owned = name.to_string();
    let songs = match tokio::task::spawn_blocking(move || {
        let mut paths = read_playlist(&playlist_dir_clone, &name_owned).map_err(|_| {
            ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "load", "No such playlist")
        })?;

        // Apply the optional range, clamping like MPD's playlist enumerator:
        // a start beyond the playlist's length simply yields nothing, not an
        // error (`playlist_load_into_queue` never validates the range).
        if let Some((start, end)) = range {
            let total = paths.len();
            let start = (start as usize).min(total);
            let end = (end as usize).min(total).max(start);
            paths = paths[start..end].to_vec();
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
        if let Some(pos) = resolved_position {
            for (i, song) in songs.into_iter().enumerate() {
                queue.add_at(song, Some(pos + i as u32));
            }
        } else {
            for song in songs {
                queue.add(song);
            }
        }
        // Mirrors MPD's unconditional `SetLastLoadedPlaylist(uri)` at the
        // end of `playlist_load_into_queue`: recorded once the playlist
        // file was successfully read, regardless of how many songs (if
        // any) actually matched the range/database.
        queue.set_last_loaded_playlist(name);
    }

    crate::helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

/// Load a `.cue` sheet as virtual tracks: each track is added to the queue with
/// its own playback range (start/end in seconds) so playback is restricted to
/// that segment of the underlying audio file. `position` is already an
/// absolute queue index, resolved by `resolve_insert_position`.
async fn load_cue_virtual_tracks(
    state: &AppState,
    playlist_dir: &str,
    name: &str,
    range: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    let mut tracks = match read_cue_tracks(playlist_dir, name) {
        Ok(t) => t,
        Err(_) => {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "load", "No such playlist");
        }
    };

    if let Some((start, end)) = range {
        let total = tracks.len();
        let start = (start as usize).min(total);
        let end = (end as usize).min(total).max(start);
        tracks = tracks[start..end].to_vec();
    }

    {
        let mut queue = state.queue.write().await;
        for (i, (song, song_range)) in tracks.into_iter().enumerate() {
            let pos = position.map(|p| p + i as u32);
            let id = queue.add_at(song, pos);
            queue.set_range_by_id(id, Some(song_range));
        }
        queue.set_last_loaded_playlist(name);
    }

    crate::helpers::update_playlist_version(state).await;
    ResponseBuilder::new().ok()
}

pub async fn handle_searchaddpl_command(
    state: &AppState,
    name: &str,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    position: Option<u32>,
) -> String {
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "searchaddpl", &e);
    }

    let state_db = state.clone();
    let filters = filters.to_vec();
    let sort = sort.map(|s| s.to_string());
    let songs = match tokio::task::spawn_blocking(move || {
        let db = open_db(&state_db, "searchaddpl")?;
        // `searchaddpl`'s parameters have the same meaning as `search`
        // (case-insensitive).
        let mut songs = crate::helpers::resolve_filters(&db, &filters, "searchaddpl", false)?;
        if let Some(sort_arg) = sort.as_deref() {
            match parse_sort_tag(sort_arg) {
                Some((key, descending)) => sort_songs(&mut songs, &key, descending),
                None => {
                    return Err(ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "searchaddpl",
                        "Unknown sort tag",
                    ));
                }
            }
        }
        Ok(songs)
    })
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return e,
        Err(_) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, "searchaddpl", "internal error"),
    };
    let new_paths: Vec<String> = apply_range(&songs, window)
        .iter()
        .map(|s| s.path.to_string())
        .collect();

    let state = state.clone();
    let name = name.to_string();
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
        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        let mut paths = if pl_path.exists() {
            read_m3u_playlist(&playlist_dir, &name).unwrap_or_default()
        } else {
            vec![]
        };

        if let Some(pos) = position
            && pos as usize > paths.len()
        {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "searchaddpl", "Bad position");
        }

        if let Some(pos) = position {
            for (i, p) in new_paths.into_iter().enumerate() {
                paths.insert(pos as usize + i, p);
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "listplaylist", &e);
    }

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
        let paths = match read_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "listplaylist",
                    "No such playlist",
                );
            }
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "listplaylistinfo", &e);
    }

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

        let paths = match read_playlist(&playlist_dir, &name) {
            Ok(p) => p,
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "listplaylistinfo",
                    "No such playlist",
                );
            }
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
                    resp.song(&songs[0], None, None, None);
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistadd", &e);
    }

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

        let pl_path = Path::new(&playlist_dir).join(format!("{name}.m3u"));
        let mut paths = if pl_path.exists() {
            read_m3u_playlist(&playlist_dir, &name).unwrap_or_default()
        } else {
            vec![]
        };

        // Bound-check POSITION against the playlist's current size ahead of
        // any lookup, matching MPD's `handle_playlistadd_position` (checked
        // once, before either the scheme-URI or database-search path runs).
        if let Some(pos) = position
            && pos as usize > paths.len()
        {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistadd", "Bad position");
        }

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

        // Collect new paths to insert/append (matching MPD behavior)
        let new_paths: Vec<String> = songs.iter().map(|s| s.path.to_string()).collect();
        if let Some(pos) = position {
            let pos = pos as usize;
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistclear", &e);
    }

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

pub async fn handle_playlistdelete_command(
    state: &AppState,
    name: &str,
    range: (u32, u32),
) -> String {
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistdelete", &e);
    }

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
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "playlistdelete",
                    "No such playlist",
                );
            }
        };
        // Mirrors `RangeArg::CheckClip`: only the start bound is validated;
        // the end is silently clipped to the playlist's length (so e.g. a
        // single index one past the end is accepted as a no-op removal).
        let (start, end) = range;
        let start = start as usize;
        if start > paths.len() {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistdelete", "Bad song index");
        }
        let end = (end as usize).min(paths.len());
        paths.drain(start..end);
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
    from: (u32, u32),
    to: u32,
) -> String {
    // MPD doesn't support an open-ended FROM range for playlistmove, and an
    // empty range or a move to its own start position succeeds as a no-op
    // without even checking the playlist name/existence (MPD's comment:
    // "this doesn't check whether the playlist exists, but what the
    // hell.."). Both checks run before `PlaylistFileEditor` (which is what
    // validates the name) is ever constructed, so they must precede our own
    // name validation too.
    if from.1 == u32::MAX {
        return ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "playlistmove",
            "Open-ended range not supported",
        );
    }
    if from.0 >= from.1 || from.0 == to {
        return ResponseBuilder::new().ok();
    }

    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistmove", &e);
    }

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
            Err(_) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "playlistmove",
                    "No such playlist",
                );
            }
        };
        // Mirrors `PlaylistFileEditor::MoveIndex`: `src.end` must fit inside
        // the playlist, and `dest` is bounded by the size *after* removing
        // the moved range (not the original length).
        let (start, end) = from;
        let (start, end) = (start as usize, end as usize);
        let to = to as usize;
        let total = paths.len();
        let count = end - start;
        if end > total || to > total - count {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistmove", "Bad range");
        }
        let moved: Vec<String> = paths.drain(start..end).collect();
        paths.splice(to..to, moved);
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "rm", &e);
    }

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
    if let Err(e) = validate_playlist_name(from) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "rename", &e);
    }
    if let Err(e) = validate_playlist_name(to) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "rename", &e);
    }

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
    filters: &[(String, String)],
    window: Option<(u32, u32)>,
) -> String {
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "searchplaylist", &e);
    }
    if filters.is_empty() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "searchplaylist", "missing arguments");
    }

    let state = state.clone();
    let name = name.to_string();
    let filters = filters.to_vec();
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
        let paths = match read_playlist(&playlist_dir, &name) {
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

        // Same filter grammar and case-insensitivity as `search` (MPD's
        // `filter.Parse(args, /*fold_case=*/true)`), matched against the DB
        // then intersected with this playlist's own songs so the printed
        // "Pos:" reflects the playlist's order, not a DB query order.
        let matched = match crate::helpers::resolve_filters(&db, &filters, "searchplaylist", false)
        {
            Ok(s) => s,
            Err(e) => return e,
        };
        let matched_by_path: std::collections::HashMap<&str, &rmpd_core::song::Song> =
            matched.iter().map(|s| (s.path.as_str(), s)).collect();

        // Mirrors `playlist_provider_search_print`: `position` walks every
        // playlist entry regardless of match; `window` skips/limits matches.
        let (win_start, win_end) = window.unwrap_or((0, u32::MAX));
        let mut skip = win_start as u64;
        let mut remaining = (win_end as u64).saturating_sub(win_start as u64);

        let mut resp = ResponseBuilder::new();
        for (pos, path) in paths.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            let Some(song) = matched_by_path.get(path.as_str()).copied() else {
                continue;
            };
            if skip > 0 {
                skip -= 1;
                continue;
            }
            resp.song(song, Some(pos as u32), None, None);
            remaining -= 1;
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
    if let Err(e) = validate_playlist_name(name) {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "playlistlength", &e);
    }

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
        let paths = match read_playlist(&playlist_dir, &name) {
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
