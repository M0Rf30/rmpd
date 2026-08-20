//! Database and library browsing command handlers

use tracing::{debug, error};

use crate::helpers;
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

/// Maps a database directory-lookup failure to the right ACK code, mirroring
/// MPD's `CommandError.cxx` exception mapping: `DatabaseErrorCode::NOT_FOUND`
/// (a directory that isn't in the database tree) becomes `ACK_ERROR_NO_EXIST`
/// (50), while any other failure (a real SQL/IO error) keeps `ACK_ERROR_SYS`
/// (52) — MPD reserves 52 for genuine `std::system_error` failures, not
/// "not found". `rmpd_library::Database::list_directory`/`walk_recursive`
/// only ever produce the literal `"No such directory"` message for the
/// former; any other message is the latter.
fn directory_lookup_ack(command: &str, err: &rmpd_core::error::RmpdError) -> String {
    let msg = err.to_string();
    let msg = msg.strip_prefix("Library error: ").unwrap_or(&msg);
    if msg == "No such directory" {
        ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, command, msg)
    } else {
        ResponseBuilder::error(ACK_ERROR_SYS, 0, command, msg)
    }
}

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_NO_EXIST, ACK_ERROR_SYS, add_songs_at_position, apply_range,
    filter_parse_ack, format_iso8601_timestamp, open_db, parse_filter_args, parse_sort_tag,
    resolve_add_position, sort_songs,
};

async fn handle_find_search_core(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    case_sensitive: bool,
) -> String {
    let cmd = if case_sensitive { "find" } else { "search" };
    let state = state.clone();
    let filters = filters.to_vec();
    let sort = sort.map(|s| s.to_string());
    match tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, cmd) {
            Ok(d) => d,
            Err(e) => return e,
        };

        let mut songs = match helpers::resolve_filters(&db, &filters, cmd, case_sensitive) {
            Ok(s) => s,
            Err(e) => return e,
        };

        if let Some(sort_arg) = sort.as_deref() {
            match parse_sort_tag(sort_arg) {
                Some((key, descending)) => sort_songs(&mut songs, &key, descending),
                None => {
                    return ResponseBuilder::error(ACK_ERROR_ARG, 0, cmd, "Unknown sort tag");
                }
            }
        }

        let filtered = apply_range(&songs, window);
        let mut resp = ResponseBuilder::new();
        for song in filtered {
            resp.song(song, None, None, None);
        }
        resp.ok()
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "internal error"),
    }
}

pub async fn handle_find_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    handle_find_search_core(state, filters, sort, window, true).await
}

pub async fn handle_search_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    handle_find_search_core(state, filters, sort, window, false).await
}

/// Nested tag-value tree for `list`'s (repeatable) grouping, mirroring
/// MPD's `RecursiveMap<std::string>` (`std::map<std::string,
/// RecursiveMap<std::string>>`): each level groups by one tag type in
/// `names` order (innermost = the requested tag), sorted byte-wise like
/// `std::map` — MPD applies no locale collation here.
#[derive(Default)]
#[allow(clippy::disallowed_types)] // `list ... group` output must be key-ordered
struct TagTree(std::collections::BTreeMap<String, TagTree>);

impl TagTree {
    /// `values[0]` is the set of values for this level's tag (usually one,
    /// but a multi-valued tag fans out into multiple branches, each
    /// continuing with the same remaining `values[1..]`).
    fn insert_path(&mut self, values: &[Vec<&str>]) {
        let Some((first, rest)) = values.split_first() else {
            return;
        };
        let keys: &[&str] = if first.is_empty() { &[""] } else { first };
        for &key in keys {
            self.0.entry(key.to_string()).or_default().insert_path(rest);
        }
    }
}

/// Prints `name: key` for every entry at this level, recursing into deeper
/// levels. `window` restricts only the outermost level (matches MPD, which
/// passes `RangeArg::All()` to every recursive call after the first).
fn print_tag_tree(
    resp: &mut ResponseBuilder,
    names: &[&str],
    tree: &TagTree,
    window: Option<(u32, u32)>,
) {
    let Some((&name, rest_names)) = names.split_first() else {
        return;
    };
    let (start, end) = window.unwrap_or((0, u32::MAX));
    for (i, (key, child)) in tree.0.iter().enumerate() {
        let pos = i as u32;
        if pos < start {
            continue;
        }
        if pos >= end {
            break;
        }
        resp.field(name, key);
        if !rest_names.is_empty() {
            print_tag_tree(resp, rest_names, child, None);
        }
    }
}

pub async fn handle_list_command(
    state: &AppState,
    tag: &str,
    filters: &[(String, String)],
    groups: &[String],
    window: Option<(u32, u32)>,
) -> String {
    let state = state.clone();
    let tag = tag.to_string();
    let filters = filters.to_vec();
    let groups = groups.to_vec();
    match tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "list") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let query_songs = |db: &rmpd_library::Database| {
            if filters.is_empty() {
                db.get_all_songs().map_err(|e| {
                    ResponseBuilder::error(ACK_ERROR_SYS, 0, "list", &format!("query error: {e}"))
                })
            } else {
                // `list`'s filter is always parsed case-sensitively,
                // regardless of tag/value casing (MPD hardcodes
                // `fold_case=false` for both call sites in handle_list).
                let expr =
                    parse_filter_args(&filters, false).map_err(|e| filter_parse_ack("list", &e))?;
                db.find_songs_filter(&expr).map_err(|e| {
                    ResponseBuilder::error(ACK_ERROR_SYS, 0, "list", &format!("query error: {e}"))
                })
            }
        };

        // Deprecated `list file`/`list filename`: lists matching file URIs.
        // Never grouped/sorted in MPD (handle_list_file has no such concept).
        if tag.eq_ignore_ascii_case("file") || tag.eq_ignore_ascii_case("filename") {
            let songs = match query_songs(&db) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let filtered = apply_range(&songs, window);
            let mut resp = ResponseBuilder::new();
            for song in filtered {
                resp.field("file", &song.path);
            }
            return resp.ok();
        }

        let tag_lower = tag.to_lowercase();
        if rmpd_core::song::canonical_tag_name(&tag_lower) == "Unknown" {
            return ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                "list",
                &format!("Unknown tag type: {tag}"),
            );
        }

        let mut group_lowers: Vec<String> = Vec::with_capacity(groups.len());
        for g in &groups {
            let gl = g.to_lowercase();
            if rmpd_core::song::canonical_tag_name(&gl) == "Unknown" {
                return ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    "list",
                    &format!("Unknown tag type: {g}"),
                );
            }
            if gl == tag_lower || group_lowers.contains(&gl) {
                return ResponseBuilder::error(ACK_ERROR_ARG, 0, "list", "Conflicting group");
            }
            group_lowers.push(gl);
        }

        let songs = match query_songs(&db) {
            Ok(s) => s,
            Err(e) => return e,
        };

        // tag_types order: groups first, then the requested tag (the
        // requested tag is always the innermost/leaf level).
        let mut names = group_lowers;
        names.push(tag_lower);

        let mut tree = TagTree::default();
        for song in &songs {
            let value_sets: Vec<Vec<&str>> = names
                .iter()
                .map(|n| song.tag_values_with_fallback(n))
                .collect();
            tree.insert_path(&value_sets);
        }

        let display_names: Vec<&str> = names
            .iter()
            .map(|n| rmpd_core::song::canonical_tag_name(n))
            .collect();
        let mut resp = ResponseBuilder::new();
        print_tag_tree(&mut resp, &display_names, &tree, window);
        resp.ok()
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "list", "internal error"),
    }
}

async fn handle_count_core(
    state: &AppState,
    filters: &[(String, String)],
    group: Option<&str>,
    fold_case: bool,
) -> String {
    let cmd = if fold_case { "searchcount" } else { "count" };
    let state = state.clone();
    let filters = filters.to_vec();
    let group = group.map(|s| s.to_string());
    match tokio::task::spawn_blocking(move || {
        // MPD validates the group tag before touching the filter/database.
        let group_lower = match group.as_deref() {
            Some(g) => {
                let gl = g.to_lowercase();
                if rmpd_core::song::canonical_tag_name(&gl) == "Unknown" {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        cmd,
                        &format!("Unknown tag type: {g}"),
                    );
                }
                Some(gl)
            }
            None => None,
        };

        let db = match open_db(&state, cmd) {
            Ok(d) => d,
            Err(e) => return e,
        };

        // Bare "count"/"searchcount" with no filter and no group is an
        // error; "count group TAG" (empty filter, grouped) counts everything.
        if filters.is_empty() && group_lower.is_none() {
            return ResponseBuilder::error(
                ACK_ERROR_ARG,
                0,
                cmd,
                &format!("too few arguments for \"{cmd}\""),
            );
        }
        // Note: an empty string value is valid (e.g. `count title ""` finds
        // songs with a blank title).

        let songs = if filters.is_empty() {
            match db.get_all_songs() {
                Ok(s) => s,
                Err(e) => {
                    return ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        cmd,
                        &format!("query error: {e}"),
                    );
                }
            }
        } else {
            match parse_filter_args(&filters, fold_case) {
                Ok(expr) => match db.find_songs_filter(&expr) {
                    Ok(s) => s,
                    Err(e) => {
                        return ResponseBuilder::error(
                            ACK_ERROR_SYS,
                            0,
                            cmd,
                            &format!("query error: {e}"),
                        );
                    }
                },
                Err(e) => return filter_parse_ack(cmd, &e),
            }
        };

        let mut resp = ResponseBuilder::new();

        if let Some(group_tag) = group_lower.as_deref() {
            // Group by specified tag — sorted output to match MPD
            use std::collections::HashMap;
            let mut groups: HashMap<String, (usize, f64)> = HashMap::new();
            for song in &songs {
                let vals = song.tag_values_with_fallback(group_tag);
                let vals: Vec<&str> = if vals.is_empty() { vec![""] } else { vals };
                for group_value in vals {
                    let entry = groups.entry(group_value.to_string()).or_insert((0, 0.0));
                    entry.0 += 1;
                    if let Some(duration) = song.duration {
                        entry.1 += duration.as_secs_f64();
                    }
                }
            }
            // Sort by tag value (MPD uses std::map which sorts lexicographically)
            let mut sorted: Vec<_> = groups.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            let tag_key = rmpd_core::song::canonical_tag_name(group_tag);
            for (value, (count, playtime)) in &sorted {
                resp.field(tag_key, value);
                resp.field("songs", count);
                resp.field("playtime", playtime.floor() as u64);
            }
        } else {
            // No grouping - return totals
            // Sum fractional seconds, then truncate (MPD uses duration_cast<seconds> = truncation)
            let total_duration: u64 = songs
                .iter()
                .filter_map(|s| s.duration)
                .map(|d| d.as_secs_f64())
                .sum::<f64>()
                .floor() as u64;
            resp.field("songs", songs.len());
            resp.field("playtime", total_duration);
        }

        resp.ok()
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "internal error"),
    }
}

pub async fn handle_count_command(
    state: &AppState,
    filters: &[(String, String)],
    group: Option<&str>,
) -> String {
    handle_count_core(state, filters, group, false).await
}

pub async fn handle_update_command(state: &AppState, path: Option<&str>, discard: bool) -> String {
    let cmd = if discard { "rescan" } else { "update" };
    if state.db_path.is_none() {
        return ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "database not configured");
    }
    let Some(music_dir) = state.music_dir.as_deref() else {
        return ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "music directory not configured");
    };

    // Backward-compat aliases for "the whole music directory" (MPD 0.15).
    let path = match path {
        Some(p) if !p.is_empty() && p != "/" => p,
        _ => "",
    };
    if !path.is_empty() {
        if !rmpd_core::path::uri_safe_local(path) {
            return ResponseBuilder::error(ACK_ERROR_ARG, 0, cmd, "Malformed path");
        }
        if !std::path::Path::new(music_dir).join(path).exists() {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, cmd, "No such directory");
        }
        // NOTE: the scan below always covers the whole music directory.
        // Scoping it to `path` would require decoupling `Scanner`'s
        // `music_directory` (used for relative-path computation) from its
        // scan root, which `scan_directory` currently conflates — deferred,
        // not a surgical fix. `path` is still validated and still gets
        // rescanned, just as part of the full tree rather than exclusively.
    }

    // Also sync enabled music sources.
    state.spawn_source_sync();

    match state.spawn_library_update(discard).await {
        Some(job_id) => {
            let mut resp = ResponseBuilder::new();
            resp.field("updating_db", job_id);
            resp.ok()
        }
        None => ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "database not configured"),
    }
}

/// Derive a plausible cover filename from a MIME type, for the synthetic
/// `file` field on source-backed (remote) albumart responses where there is
/// no real on-disk directory to report a filename from.
fn synth_cover_filename(uri: &str, mime_type: &str) -> String {
    let ext = match mime_type {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "jpg",
    };
    match uri.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/cover.{ext}"),
        None => format!("cover.{ext}"),
    }
}

pub async fn handle_albumart_command(state: &AppState, uri: &str, offset: usize) -> Response {
    debug!("albumart command: uri=[{}], offset={}", uri, offset);

    // Source-backed mount-style paths (e.g. `alarm-music/…`): artwork is fetched
    // from the source server (once) and cached locally — never read from a file.
    if state.sources.owns_path(uri) {
        let state_open = state.clone();
        let db = match tokio::task::spawn_blocking(move || open_db(&state_open, "albumart")).await {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => return Response::Text(e),
            Err(_) => {
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "albumart",
                    "internal error",
                ));
            }
        };

        let uri_owned = uri.to_string();
        let (extractor, is_cached) = match tokio::task::spawn_blocking(move || {
            let extractor = rmpd_library::AlbumArtExtractor::new(db);
            let cached = extractor.is_cached(&uri_owned);
            (extractor, cached)
        })
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "albumart",
                    "internal error",
                ));
            }
        };

        let extractor = if !is_cached && let Ok(Some(bytes)) = state.sources.cover_art(uri).await {
            let uri_owned = uri.to_string();
            match tokio::task::spawn_blocking(move || {
                let _ = extractor.cache_external(&uri_owned, &bytes);
                extractor
            })
            .await
            {
                Ok(e) => e,
                Err(_) => {
                    return Response::Text(ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        "albumart",
                        "internal error",
                    ));
                }
            }
        } else {
            extractor
        };

        let uri_owned = uri.to_string();
        return match tokio::task::spawn_blocking(move || {
            extractor.get_artwork(&uri_owned, "", offset)
        })
        .await
        {
            Ok(Ok(rmpd_library::ArtLookup::Found(artwork))) => {
                let file_field = synth_cover_filename(uri, &artwork.mime_type);
                let mut resp = ResponseBuilder::new();
                resp.field("file", &file_field);
                resp.field("size", artwork.total_size);
                resp.binary_field("binary", &artwork.data);
                Response::Binary(resp.to_binary_response())
            }
            Ok(Ok(rmpd_library::ArtLookup::NotFound)) => Response::Text(ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "albumart",
                "No file exists",
            )),
            Ok(Ok(rmpd_library::ArtLookup::OffsetTooLarge)) => Response::Text(
                ResponseBuilder::error(ACK_ERROR_ARG, 0, "albumart", "Offset too large"),
            ),
            Ok(Err(_)) => Response::Text(ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "albumart",
                "No file exists",
            )),
            Err(_) => Response::Text(ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "albumart",
                "internal error",
            )),
        };
    }

    // Local files: `albumart` only looks at a standalone cover image
    // (cover.png/.jpg/.jxl/.webp) in the song's directory — it never reads
    // embedded tag pictures. That's `readpicture`'s job.
    let absolute_path = if uri.starts_with('/') {
        uri.to_string()
    } else {
        match &state.music_dir {
            Some(music_dir) => format!("{music_dir}/{uri}"),
            None => {
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "albumart",
                    "music directory not configured",
                ));
            }
        }
    };
    let dir = match std::path::Path::new(&absolute_path).parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return Response::Text(ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "albumart",
                "No file exists",
            ));
        }
    };
    let uri_dir = uri.rsplit_once('/').map(|(d, _)| d.to_string());

    match tokio::task::spawn_blocking(move || rmpd_library::find_external_cover(&dir, offset)).await
    {
        Ok(rmpd_library::ArtLookup::Found(art)) => {
            let file_field = match &uri_dir {
                Some(d) => format!("{d}/{}", art.filename),
                None => art.filename.to_string(),
            };
            let mut resp = ResponseBuilder::new();
            resp.field("file", &file_field);
            resp.field("size", art.total_size);
            resp.binary_field("binary", &art.data);
            Response::Binary(resp.to_binary_response())
        }
        Ok(rmpd_library::ArtLookup::NotFound) => Response::Text(ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "albumart",
            "No file exists",
        )),
        Ok(rmpd_library::ArtLookup::OffsetTooLarge) => Response::Text(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "albumart",
            "Offset too large",
        )),
        Err(_) => Response::Text(ResponseBuilder::error(
            ACK_ERROR_SYS,
            0,
            "albumart",
            "internal error",
        )),
    }
}

pub async fn handle_readpicture_command(state: &AppState, uri: &str, offset: usize) -> Response {
    // readpicture returns embedded pictures from audio files.
    // Unlike albumart: file-not-found -> "No such song", no picture -> OK (empty)
    let state_open = state.clone();
    let db = match tokio::task::spawn_blocking(move || open_db(&state_open, "readpicture")).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return Response::Text(e),
        Err(_) => {
            return Response::Text(ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "readpicture",
                "internal error",
            ));
        }
    };

    // Source-backed mount-style paths: serve the cached server cover art; "no
    // art" is an empty OK (matching readpicture semantics), not an error.
    if state.sources.owns_path(uri) {
        let uri_owned = uri.to_string();
        let (extractor, is_cached) = match tokio::task::spawn_blocking(move || {
            let extractor = rmpd_library::AlbumArtExtractor::new(db);
            let cached = extractor.is_cached(&uri_owned);
            (extractor, cached)
        })
        .await
        {
            Ok(v) => v,
            Err(_) => {
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_SYS,
                    0,
                    "readpicture",
                    "internal error",
                ));
            }
        };

        let extractor = if !is_cached && let Ok(Some(bytes)) = state.sources.cover_art(uri).await {
            let uri_owned = uri.to_string();
            match tokio::task::spawn_blocking(move || {
                let _ = extractor.cache_external(&uri_owned, &bytes);
                extractor
            })
            .await
            {
                Ok(e) => e,
                Err(_) => {
                    return Response::Text(ResponseBuilder::error(
                        ACK_ERROR_SYS,
                        0,
                        "readpicture",
                        "internal error",
                    ));
                }
            }
        } else {
            extractor
        };

        let uri_owned = uri.to_string();
        return match tokio::task::spawn_blocking(move || {
            extractor.get_artwork(&uri_owned, "", offset)
        })
        .await
        {
            Ok(Ok(rmpd_library::ArtLookup::Found(artwork))) => {
                let mut resp = ResponseBuilder::new();
                resp.field("size", artwork.total_size);
                resp.field("type", &artwork.mime_type);
                resp.binary_field("binary", &artwork.data);
                Response::Binary(resp.to_binary_response())
            }
            Ok(Ok(rmpd_library::ArtLookup::NotFound)) => {
                Response::Text(ResponseBuilder::new().ok())
            }
            Ok(Err(_)) => Response::Text(ResponseBuilder::new().ok()),
            Ok(Ok(rmpd_library::ArtLookup::OffsetTooLarge)) => Response::Text(
                ResponseBuilder::error(ACK_ERROR_ARG, 0, "readpicture", "Bad file offset"),
            ),
            Err(_) => Response::Text(ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "readpicture",
                "internal error",
            )),
        };
    }

    let absolute_path = if uri.starts_with('/') {
        uri.to_string()
    } else {
        match &state.music_dir {
            Some(music_dir) => format!("{music_dir}/{uri}"),
            None => {
                return Response::Text(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "readpicture",
                    "music directory not configured",
                ));
            }
        }
    };

    let uri_owned = uri.to_string();
    let absolute_path_for_check = absolute_path.clone();
    match tokio::task::spawn_blocking(move || {
        let extractor = rmpd_library::AlbumArtExtractor::new(db);
        extractor.get_artwork(&uri_owned, &absolute_path, offset)
    })
    .await
    {
        Ok(Ok(rmpd_library::ArtLookup::Found(artwork))) => {
            let mut resp = ResponseBuilder::new();
            resp.field("size", artwork.total_size);
            resp.field("type", &artwork.mime_type);
            resp.binary_field("binary", &artwork.data);
            Response::Binary(resp.to_binary_response())
        }
        Ok(Ok(rmpd_library::ArtLookup::NotFound)) => {
            // File exists but no embedded picture — return empty OK
            Response::Text(ResponseBuilder::new().ok())
        }
        Ok(Ok(rmpd_library::ArtLookup::OffsetTooLarge)) => Response::Text(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            "readpicture",
            "Bad file offset",
        )),
        Ok(Err(_)) => {
            // Check if the file actually exists
            // If it does, treat the error as "no embedded picture" -> OK
            // If it doesn't, return "No such song"
            if std::path::Path::new(&absolute_path_for_check).exists() {
                Response::Text(ResponseBuilder::new().ok())
            } else {
                Response::Text(ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "readpicture",
                    "No such song",
                ))
            }
        }
        Err(_) => Response::Text(ResponseBuilder::error(
            ACK_ERROR_SYS,
            0,
            "readpicture",
            "internal error",
        )),
    }
}

// Queue inspection
pub async fn handle_currentsong_command(state: &AppState) -> String {
    let status = state.status.read().await;
    let queue = state.queue.read().await;

    if let Some(current) = status.current_song
        && let Some(item) = queue.get(current.position)
    {
        let mut resp = ResponseBuilder::new();
        // For remote streams, surface the live ICY "now playing" title as Title.
        if rmpd_core::path::is_uri(item.song.path.as_str())
            && let Some(title) = state.stream_title.read().await.clone()
        {
            let mut song = (*item.song).clone();
            if let Some(slot) = song.tags.iter_mut().find(|(k, _)| k == "title") {
                slot.1 = title;
            } else {
                song.tags.push((std::borrow::Cow::Borrowed("title"), title));
            }
            resp.song(&song, Some(current.position), Some(current.id), item.range);
        } else {
            resp.song(
                &item.song,
                Some(current.position),
                Some(current.id),
                item.range,
            );
        }
        return resp.ok();
    }

    // No current song
    ResponseBuilder::new().ok()
}

// Browsing commands
pub async fn handle_lsinfo_command(state: &AppState, path: Option<&str>) -> String {
    let state = state.clone();
    let path = path.map(|s| s.to_string());
    match tokio::task::spawn_blocking(move || {
        let path = path.as_deref();
        let db = match open_db(&state, "lsinfo") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let path_str = path.unwrap_or("");

        // First check if path refers to a single file (song), matching MPD behavior
        // where `lsinfo <file>` returns just that file's info.
        if !path_str.is_empty() && path_str != "/" {
            match db.get_song_by_path(path_str) {
                Ok(Some(song)) => {
                    let mut resp = ResponseBuilder::new();
                    let music_dir = state.music_dir.as_deref();
                    let display_path = strip_music_dir_prefix(song.path.as_str(), music_dir);
                    let mut display_song = song.clone();
                    display_song.path = display_path.into();
                    resp.song(&display_song, None, None, None);
                    return resp.ok();
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }

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
                    resp.song(&display_song, None, None, None);
                }
                for (dir, mtime) in &listing.directories {
                    let display_dir = strip_music_dir_prefix(dir, music_dir);
                    resp.field("directory", display_dir);
                    if *mtime > 0 {
                        let ts = format_iso8601_timestamp(*mtime);
                        resp.field("Last-Modified", &ts);
                    }
                }

                // For root directory, also list playlists (read from filesystem, matching MPD behavior)
                if (path_str.is_empty() || path_str == "/")
                    && let Some(playlist_dir) = &state.playlist_dir
                {
                    let mut entries: Vec<(String, i64)> = Vec::new();
                    if let Ok(dir) = std::fs::read_dir(playlist_dir) {
                        for entry in dir.flatten() {
                            let fpath = entry.path();
                            if fpath.extension().and_then(|e| e.to_str()) == Some("m3u")
                                && let Some(stem) = fpath.file_stem().and_then(|s| s.to_str())
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
                    }
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    for (name, mtime) in &entries {
                        resp.field("playlist", name);
                        let timestamp_str = format_iso8601_timestamp(*mtime);
                        resp.field("Last-Modified", &timestamp_str);
                    }
                }

                resp.ok()
            }
            Err(e) => directory_lookup_ack("lsinfo", &e),
        }
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "lsinfo", "internal error"),
    }
}

pub async fn handle_listall_command(state: &AppState, path: Option<&str>) -> String {
    let state = state.clone();
    let path = path.map(|s| s.to_string());
    match tokio::task::spawn_blocking(move || {
        let path = path.as_deref();
        let db = match open_db(&state, "listall") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let path_str = path.unwrap_or("");
        let mut resp = ResponseBuilder::new();

        // If a specific path is given, check if it's a file first
        if !path_str.is_empty() && path_str != "/" {
            match db.get_song_by_path(path_str) {
                Ok(Some(song)) => {
                    // MPD returns just the file entry for a file path
                    resp.field("file", &song.path);
                    return resp.ok();
                }
                Ok(None) => {}
                Err(_) => {}
            }
            // It's a directory path: emit the directory itself first (MPD behavior)
            resp.field("directory", path_str);
        }

        let result = db.walk_recursive(path_str, &mut |entry| {
            match entry {
                rmpd_library::WalkEntry::Song(song) => {
                    resp.field("file", &song.path);
                }
                rmpd_library::WalkEntry::Directory(dir, _mtime) => {
                    resp.field("directory", dir);
                }
            }
            Ok(())
        });

        match result {
            Ok(()) => resp.ok(),
            Err(e) => directory_lookup_ack("listall", &e),
        }
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listall", "internal error"),
    }
}

pub async fn handle_listallinfo_command(state: &AppState, path: Option<&str>) -> String {
    let state = state.clone();
    let path = path.map(|s| s.to_string());
    match tokio::task::spawn_blocking(move || {
        let path = path.as_deref();
        let db = match open_db(&state, "listallinfo") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let path_str = path.unwrap_or("");
        let mut resp = ResponseBuilder::new();

        // If a specific path is given, check if it's a file first
        if !path_str.is_empty() && path_str != "/" {
            match db.get_song_by_path(path_str) {
                Ok(Some(song)) => {
                    // MPD returns just the file's full info for a file path
                    resp.song(&song, None, None, None);
                    return resp.ok();
                }
                Ok(None) => {}
                Err(_) => {}
            }
            // It's a directory path: emit the directory itself + Last-Modified first (MPD behavior)
            resp.field("directory", path_str);
            if let Ok(Some(mtime)) = db.get_directory_mtime(path_str)
                && mtime > 0
            {
                resp.field("Last-Modified", format_iso8601_timestamp(mtime));
            }
        }

        let result = db.walk_recursive(path_str, &mut |entry| {
            match entry {
                rmpd_library::WalkEntry::Song(song) => {
                    resp.song(song, None, None, None);
                }
                rmpd_library::WalkEntry::Directory(dir, mtime) => {
                    resp.field("directory", dir);
                    if mtime > 0 {
                        resp.field("Last-Modified", format_iso8601_timestamp(mtime));
                    }
                }
            }
            Ok(())
        });

        match result {
            Ok(()) => resp.ok(),
            Err(e) => directory_lookup_ack("listallinfo", &e),
        }
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listallinfo", "internal error"),
    }
}

async fn handle_match_add_core(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    position: Option<crate::parser::InsertPosition>,
    case_sensitive: bool,
) -> String {
    let cmd = if case_sensitive {
        "findadd"
    } else {
        "searchadd"
    };
    let state_db = state.clone();
    let filters = filters.to_vec();
    let sort = sort.map(|s| s.to_string());
    let songs = match tokio::task::spawn_blocking(move || {
        let db = open_db(&state_db, cmd)?;
        let mut songs = helpers::resolve_filters(&db, &filters, cmd, case_sensitive)?;
        if let Some(sort_arg) = sort.as_deref() {
            match parse_sort_tag(sort_arg) {
                Some((key, descending)) => sort_songs(&mut songs, &key, descending),
                None => {
                    return Err(ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        cmd,
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
        Err(_) => return ResponseBuilder::error(ACK_ERROR_SYS, 0, cmd, "internal error"),
    };

    let position = match resolve_add_position(state, position, cmd).await {
        Ok(p) => p,
        Err(e) => return e,
    };

    let windowed = apply_range(&songs, window).to_vec();
    match add_songs_at_position(state, windowed, position, cmd).await {
        Ok(()) => ResponseBuilder::new().ok(),
        Err(e) => e,
    }
}

pub async fn handle_searchadd_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    position: Option<crate::parser::InsertPosition>,
) -> String {
    handle_match_add_core(state, filters, sort, window, position, false).await
}

pub async fn handle_findadd_command(
    state: &AppState,
    filters: &[(String, String)],
    sort: Option<&str>,
    window: Option<(u32, u32)>,
    position: Option<crate::parser::InsertPosition>,
) -> String {
    handle_match_add_core(state, filters, sort, window, position, true).await
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

        let path_owned = path.to_string();
        let fs_result = tokio::task::spawn_blocking(move || {
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
                    Some(resp.ok())
                }
                Err(e) => {
                    // If not found or not a directory, return MPD-style error immediately
                    // MPD uses ACK_ERROR_SYS (52) with message format:
                    // "Failed to open {path}: {os error}"
                    if !path_owned.is_empty() {
                        Some(ResponseBuilder::error(
                            52, // ACK_ERROR_SYS
                            0,
                            "listfiles",
                            // Strip the " (os error N)" suffix from Rust's error message
                            &format!(
                                "Failed to open {}: {}",
                                full_path.display(),
                                e.to_string().split(" (os error ").next().unwrap_or(""),
                            ),
                        ))
                    } else {
                        // For empty path (root), fall through to DB-based listing
                        None
                    }
                }
            }
        })
        .await;

        match fs_result {
            Ok(Some(resp)) => return resp,
            Ok(None) => {} // For empty path (root), fall through to DB-based listing
            Err(_) => {
                return ResponseBuilder::error(ACK_ERROR_SYS, 0, "listfiles", "internal error");
            }
        }
    }

    // Fallback: use database listing when music_dir is not available
    let state_db = state.clone();
    let path_owned = path.to_string();
    let music_dir_owned = state.music_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let db = match open_db(&state_db, "listfiles") {
            Ok(d) => d,
            Err(e) => return e,
        };
        match db.list_directory(&path_owned) {
            Ok(listing) => {
                let mut resp = ResponseBuilder::new();
                let music_dir = music_dir_owned.as_deref();
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
            Err(e) => directory_lookup_ack("listfiles", &e),
        }
    })
    .await
    {
        Ok(s) => s,
        Err(_) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "listfiles", "internal error"),
    }
}

/// Count search results with optional grouping (case-insensitive, like
/// `search`). Parameters have the same meaning as `count`.
pub async fn handle_searchcount_command(
    state: &AppState,
    filters: &[(String, String)],
    group: Option<&str>,
) -> String {
    handle_count_core(state, filters, group, true).await
}

/// Read file metadata comments
///
/// Reads raw key-value pairs directly from the audio file (not from the DB).
/// This matches MPD behavior which reads raw vorbis comments / ID3 frames / MP4 atoms.
pub async fn handle_readcomments_command(state: &AppState, uri: &str) -> String {
    use camino::Utf8PathBuf;
    use rmpd_library::MetadataExtractor;

    // Source-backed (remote) songs have no local file to read tags from; running
    // lofty on a mount-style path would fail. readcomments returns an empty OK.
    if state.sources.owns_path(uri) {
        return ResponseBuilder::new().ok();
    }

    // Resolve absolute path from music_dir + relative URI
    let abs_path = if let Some(music_dir) = &state.music_dir {
        let base = music_dir.trim_end_matches('/');
        format!("{base}/{uri}")
    } else {
        // Try as-is (absolute path)
        uri.to_string()
    };

    let path = Utf8PathBuf::from(&abs_path);
    if !path.exists() {
        return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "readcomments", "No such song");
    }

    match MetadataExtractor::read_raw_comments(&path) {
        Ok(pairs) => {
            let mut resp = ResponseBuilder::new();
            for (key, value) in pairs {
                // MPD's IsValidName: must start with alpha, all chars [A-Za-z_-]
                // MPD's IsValidValue: no control chars (< 0x20)
                let valid_name = !key.is_empty()
                    && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || c == '_' || c == '-');
                let valid_value = value.bytes().all(|b| b >= 0x20);
                if valid_name && valid_value {
                    resp.field(&key, &value);
                }
            }
            resp.ok()
        }
        Err(e) => {
            error!("readcomments error for {uri}: {e}");
            ResponseBuilder::error(ACK_ERROR_SYS, 0, "readcomments", "No such song")
        }
    }
}
