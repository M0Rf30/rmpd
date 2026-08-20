//! Sticker (metadata tag) command handlers
//!
//! Stickers are arbitrary key-value metadata tags that can be attached to songs.
//! They are stored persistently in the database and can be used for ratings,
//! playback counts, or any custom metadata.
//!
//! Only the `song` domain is backed by storage (rmpd's `stickers` table is
//! URI-keyed with no `type` column). MPD 0.24 also supports `playlist`,
//! `filter`, and per-tag domains (see `sticker/AllowedTags.cxx`); requests
//! for those return a clear "not supported" ACK instead of silently
//! misinterpreting the domain argument as a song URI.

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{ACK_ERROR_ARG, ACK_ERROR_NO_EXIST, ACK_ERROR_SYS, apply_range, open_db};

/// Tags MPD allows stickers on, in `sticker/AllowedTags.cxx` enum order.
const STICKER_ALLOWED_TAGS: &[&str] = &[
    "Artist",
    "Album",
    "AlbumArtist",
    "Title",
    "Genre",
    "Composer",
    "Performer",
    "Conductor",
    "Work",
    "Ensemble",
    "Location",
    "Label",
    "MUSICBRAINZ_ARTISTID",
    "MUSICBRAINZ_ALBUMID",
    "MUSICBRAINZ_ALBUMARTISTID",
    "MUSICBRAINZ_RELEASETRACKID",
    "MUSICBRAINZ_WORKID",
];

/// Reject sticker domains this build doesn't have storage for. `song` is the
/// only implemented domain; `playlist`/`filter`/tag-name domains are real
/// MPD 0.24 features we don't back yet, so callers get an honest ACK instead
/// of the domain argument being silently treated as a song URI.
fn require_song_domain(sticker_type: &str, command: &str) -> Result<(), String> {
    if sticker_type == "song" {
        return Ok(());
    }
    let recognized = sticker_type == "playlist"
        || sticker_type == "filter"
        || STICKER_ALLOWED_TAGS
            .iter()
            .any(|t| t.eq_ignore_ascii_case(sticker_type));
    if recognized {
        Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            &format!("sticker domain {sticker_type:?} is not supported (song stickers only)"),
        ))
    } else {
        Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            &format!("unknown sticker domain {sticker_type:?}"),
        ))
    }
}

/// MPD rejects `set`/`inc`/`dec` with an empty sticker name.
fn require_nonempty_name(name: &str, command: &str) -> Result<(), String> {
    if name.is_empty() {
        Err(ResponseBuilder::error(
            ACK_ERROR_ARG,
            0,
            command,
            "empty sticker name",
        ))
    } else {
        Ok(())
    }
}

/// Notify idle clients that a sticker changed, mirroring MPD's
/// `idle_add(IDLE_STICKER)` after a successful mutation.
fn notify_sticker_changed(state: &AppState) {
    state
        .event_bus
        .emit(rmpd_core::event::Event::StickerChanged);
}

fn get_sticker_i32(db: &rmpd_library::Database, uri: &str, name: &str) -> i32 {
    db.get_sticker(uri, name)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Return `Err(error_response)` when the song at `uri` does not exist in the DB.
fn require_song(db: &rmpd_library::Database, uri: &str) -> Result<(), String> {
    match db.get_song_by_path(uri) {
        Ok(None) => Err(ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "sticker",
            "No such song",
        )),
        Err(_) => Err(ResponseBuilder::error(
            ACK_ERROR_SYS,
            0,
            "sticker",
            "No such song",
        )),
        Ok(Some(_)) => Ok(()),
    }
}

/// Comparison operator for `sticker find` filters (`sticker/Match.hxx`).
#[derive(Clone, Copy)]
enum StickerCmp {
    Equals,
    LessThan,
    GreaterThan,
    EqualsInt,
    LessThanInt,
    GreaterThanInt,
    Contains,
    StartsWith,
}

/// Decode a `value` field encoded by the parser as `"op\x00val"`.
/// Returns `None` when no operator filter is present.
fn decode_sticker_filter(encoded: Option<&str>) -> Option<(StickerCmp, &str)> {
    let enc = encoded?;
    let sep = enc.find('\x00')?;
    let op = match &enc[..sep] {
        "=" => StickerCmp::Equals,
        "<" => StickerCmp::LessThan,
        ">" => StickerCmp::GreaterThan,
        "eq" => StickerCmp::EqualsInt,
        "lt" => StickerCmp::LessThanInt,
        "gt" => StickerCmp::GreaterThanInt,
        "contains" => StickerCmp::Contains,
        "starts_with" => StickerCmp::StartsWith,
        _ => return None,
    };
    Some((op, &enc[sep + 1..]))
}

/// SQLite's `CAST(text AS INT)` reads a leading optional sign and digits and
/// yields 0 when there is none; mirror that for the `eq`/`lt`/`gt` (`_INT`)
/// operators instead of Rust's stricter `str::parse`.
fn sqlite_cast_int(s: &str) -> i64 {
    let trimmed = s.trim_start();
    let mut end = 0;
    for (i, c) in trimmed.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse().unwrap_or(0)
}

/// Test whether `sticker_value` satisfies `op cmp_value`.
fn sticker_matches(op: StickerCmp, sticker_value: &str, cmp_value: &str) -> bool {
    match op {
        StickerCmp::Equals => sticker_value == cmp_value,
        StickerCmp::LessThan => sticker_value < cmp_value,
        StickerCmp::GreaterThan => sticker_value > cmp_value,
        StickerCmp::EqualsInt => sqlite_cast_int(sticker_value) == sqlite_cast_int(cmp_value),
        StickerCmp::LessThanInt => sqlite_cast_int(sticker_value) < sqlite_cast_int(cmp_value),
        StickerCmp::GreaterThanInt => sqlite_cast_int(sticker_value) > sqlite_cast_int(cmp_value),
        // SQLite's LIKE is ASCII case-insensitive, matching MPD's CONTAINS/STARTS_WITH.
        StickerCmp::Contains => sticker_value
            .to_ascii_lowercase()
            .contains(&cmp_value.to_ascii_lowercase()),
        StickerCmp::StartsWith => sticker_value
            .to_ascii_lowercase()
            .starts_with(&cmp_value.to_ascii_lowercase()),
    }
}

/// A `sticker` line with an unrecognized subcommand. MPD resolves the
/// domain (args[1]) before ever checking the subcommand (StickerCommands.cxx
/// `handle_sticker`), so an invalid domain still reports "unknown sticker
/// domain"/"not supported" here; only a valid (song) domain reaches the
/// generic "bad request".
pub fn handle_sticker_invalid_command(sticker_type: &str) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    ResponseBuilder::error(ACK_ERROR_ARG, 0, "sticker", "bad request")
}

pub async fn handle_sticker_get_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    let state = state.clone();
    let uri = uri.to_string();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "sticker") {
            Ok(d) => d,
            Err(e) => return e,
        };

        // Check song exists (MPD validates URI before sticker lookup)
        if let Err(e) = require_song(&db, &uri) {
            return e;
        }

        match db.get_sticker(&uri, &name) {
            Ok(Some(value)) => {
                let mut resp = ResponseBuilder::new();
                resp.field("sticker", format!("{name}={value}"));
                resp.ok()
            }
            Ok(None) => ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "sticker",
                &format!("no such sticker: {:?}", name),
            ),
            Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
        }
    })
    .await
    .unwrap_or_else(|_| ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"))
}

pub async fn handle_sticker_set_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    value: &str,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    if let Err(e) = require_nonempty_name(name, "sticker") {
        return e;
    }
    let state_owned = state.clone();
    let uri = uri.to_string();
    let name = name.to_string();
    let value = value.to_string();
    let (changed, response) = tokio::task::spawn_blocking(move || {
        let db = match open_db(&state_owned, "sticker") {
            Ok(d) => d,
            Err(e) => return (false, e),
        };

        if let Err(e) = require_song(&db, &uri) {
            return (false, e);
        }

        match db.set_sticker(&uri, &name, &value) {
            Ok(_) => (true, ResponseBuilder::new().ok()),
            Err(e) => (
                false,
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
            ),
        }
    })
    .await
    .unwrap_or_else(|_| {
        (
            false,
            ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"),
        )
    });
    if changed {
        notify_sticker_changed(state);
    }
    response
}

/// `delete TYPE URI` with no NAME removes every sticker for `uri`. When
/// there is nothing to delete, real MPD (StickerCommands.cxx
/// `DomainHandler::Delete`) formats `FmtError(..., "no such sticker: {:?}",
/// name)` with `name == nullptr`, which crashes the daemon (verified against
/// MPD master 793eb1219) — that is a real MPD bug, not a spec to match.
/// rmpd deliberately returns `OK` here instead of reproducing the crash or
/// inventing an error MPD itself doesn't survive to send.
pub async fn handle_sticker_delete_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: Option<&str>,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    let state_owned = state.clone();
    let uri = uri.to_string();
    let name = name.map(|s| s.to_string());
    let (changed, response) = tokio::task::spawn_blocking(move || {
        let name = name.as_deref();
        let db = match open_db(&state_owned, "sticker") {
            Ok(d) => d,
            Err(e) => return (false, e),
        };

        if let Err(e) = require_song(&db, &uri) {
            return (false, e);
        }

        // When deleting a named sticker, check it exists first (MPD returns error if not found)
        if let Some(sticker_name) = name {
            match db.get_sticker(&uri, sticker_name) {
                Ok(None) => {
                    return (
                        false,
                        ResponseBuilder::error(
                            ACK_ERROR_NO_EXIST,
                            0,
                            "sticker",
                            &format!("no such sticker: {:?}", sticker_name),
                        ),
                    );
                }
                Err(e) => {
                    return (
                        false,
                        ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
                    );
                }
                Ok(Some(_)) => {}
            }
        }

        match db.delete_sticker(&uri, name) {
            Ok(_) => (true, ResponseBuilder::new().ok()),
            Err(e) => (
                false,
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
            ),
        }
    })
    .await
    .unwrap_or_else(|_| {
        (
            false,
            ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"),
        )
    });
    if changed {
        notify_sticker_changed(state);
    }
    response
}

pub async fn handle_sticker_list_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    let state = state.clone();
    let uri = uri.to_string();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "sticker") {
            Ok(d) => d,
            Err(e) => return e,
        };

        // Check song exists
        if let Err(e) = require_song(&db, &uri) {
            return e;
        }

        match db.list_stickers(&uri) {
            Ok(stickers) => {
                let mut resp = ResponseBuilder::new();
                for (name, value) in stickers {
                    resp.field("sticker", format!("{name}={value}"));
                }
                resp.ok()
            }
            Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
        }
    })
    .await
    .unwrap_or_else(|_| ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"))
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_sticker_find_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    value: Option<&str>,
    sort: Option<&str>,
    window: Option<(u32, u32)>,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }

    // Validate `sort` up front so a bad tag fails before touching the DB.
    enum SortKey {
        Uri,
        Value,
        ValueInt,
    }
    let sort_key = match sort {
        None => None,
        Some(s) => {
            let (key, descending) = match s.strip_prefix('-') {
                Some(rest) => (rest, true),
                None => (s, false),
            };
            let key = match key {
                "uri" => SortKey::Uri,
                "value" => SortKey::Value,
                "value_int" => SortKey::ValueInt,
                _ => {
                    return ResponseBuilder::error(
                        ACK_ERROR_ARG,
                        0,
                        "sticker",
                        &format!("Unknown sort tag {:?}", s),
                    );
                }
            };
            Some((key, descending))
        }
    };

    let state = state.clone();
    let uri = uri.to_string();
    let name = name.to_string();
    let value = value.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "sticker") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let filter = decode_sticker_filter(value.as_deref());

        match db.find_stickers(&uri, &name) {
            Ok(mut results) => {
                if let Some((op, cmp_val)) = filter {
                    results
                        .retain(|(_, sticker_value)| sticker_matches(op, sticker_value, cmp_val));
                }
                if let Some((key, descending)) = sort_key {
                    match key {
                        SortKey::Uri => results.sort_by(|a, b| a.0.cmp(&b.0)),
                        SortKey::Value => results.sort_by(|a, b| a.1.cmp(&b.1)),
                        SortKey::ValueInt => {
                            results.sort_by_key(|(_, v)| sqlite_cast_int(v));
                        }
                    }
                    if descending {
                        results.reverse();
                    }
                }
                let results = apply_range(&results, window);

                let mut resp = ResponseBuilder::new();
                for (file_uri, sticker_value) in results {
                    resp.field("file", file_uri);
                    resp.field("sticker", format!("{name}={sticker_value}"));
                }
                resp.ok()
            }
            Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
        }
    })
    .await
    .unwrap_or_else(|_| ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"))
}

/// Shared core for `sticker inc` / `sticker dec`.
/// `delta` is the signed change to apply (positive for inc, negative for dec).
async fn adjust_sticker_value(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    delta: i32,
) -> String {
    if let Err(e) = require_song_domain(sticker_type, "sticker") {
        return e;
    }
    if let Err(e) = require_nonempty_name(name, "sticker") {
        return e;
    }
    let state_owned = state.clone();
    let uri = uri.to_string();
    let name = name.to_string();
    let (changed, response) = tokio::task::spawn_blocking(move || {
        let db = match open_db(&state_owned, "sticker") {
            Ok(d) => d,
            Err(e) => return (false, e),
        };
        if let Err(e) = require_song(&db, &uri) {
            return (false, e);
        }
        let new_value = get_sticker_i32(&db, &uri, &name) + delta;
        match db.set_sticker(&uri, &name, &new_value.to_string()) {
            // MPD's Inc/Dec (StickerCommands.cxx) never print the new
            // value: just OK, unlike Get/Find's `sticker_print_value`.
            Ok(_) => (true, ResponseBuilder::new().ok()),
            Err(e) => (
                false,
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
            ),
        }
    })
    .await
    .unwrap_or_else(|_| {
        (
            false,
            ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", "internal error"),
        )
    });
    if changed {
        notify_sticker_changed(state);
    }
    response
}

pub async fn handle_sticker_inc_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    delta: i32,
) -> String {
    adjust_sticker_value(state, sticker_type, uri, name, delta).await
}

pub async fn handle_sticker_dec_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    delta: i32,
) -> String {
    adjust_sticker_value(state, sticker_type, uri, name, -delta).await
}

/// `stickernames` takes no arguments: it lists every distinct sticker name
/// across all URIs (not scoped to a single song), matching MPD's
/// `SELECT DISTINCT name FROM sticker ORDER BY name`.
pub async fn handle_sticker_names_command(state: &AppState) -> String {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "stickernames") {
            Ok(d) => d,
            Err(e) => return e,
        };
        match db.list_all_sticker_names() {
            Ok(names) => {
                let mut resp = ResponseBuilder::new();
                for name in names {
                    resp.field("name", &name);
                }
                resp.ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "stickernames", &format!("Error: {e}"))
            }
        }
    })
    .await
    .unwrap_or_else(|_| ResponseBuilder::error(ACK_ERROR_SYS, 0, "stickernames", "internal error"))
}

pub async fn handle_sticker_types_command() -> String {
    // List available sticker types, matching MPD's handle_sticker_types output order.
    // MPD outputs: filter, playlist, song, then sticker_allowed_tags intersected with tag_mask.
    let mut resp = ResponseBuilder::new();
    resp.field("stickertype", "filter");
    resp.field("stickertype", "playlist");
    resp.field("stickertype", "song");
    for tag in STICKER_ALLOWED_TAGS {
        resp.field("stickertype", *tag);
    }
    resp.ok()
}

/// `stickernamestypes [TYPE]`: unique sticker names and their domain type.
/// Mirrors MPD's `handle_sticker_names_types`: `song`, `playlist`, `filter`
/// and any tag in `sticker_allowed_tags` are all valid TYPEs and simply
/// filter the listing, so a domain with no stored stickers yields a bare
/// `OK`. Only a TYPE that is not a tag name at all (`no such tag`) or a tag
/// outside the allowed set (`unsupported tag`) is an error. rmpd stores song
/// stickers only, so every other valid domain lists nothing.
pub async fn handle_sticker_namestypes_command(
    state: &AppState,
    sticker_type: Option<&str>,
) -> String {
    if let Some(t) = sticker_type
        && t != "song"
    {
        if t == "playlist" || t == "filter" {
            return ResponseBuilder::new().ok();
        }
        if STICKER_ALLOWED_TAGS.contains(&t) {
            return ResponseBuilder::new().ok();
        }
        // MPD uses the case-sensitive tag_name_parse() here, unlike the
        // `sticker` command's case-insensitive tag_name_parse_i().
        let msg = if rmpd_core::song::canonical_tag_name(t) == "Unknown" {
            format!("no such tag {t:?}")
        } else {
            format!("unsupported tag {t:?}")
        };
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "stickernamestypes", &msg);
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "stickernamestypes") {
            Ok(d) => d,
            Err(e) => return e,
        };
        match db.list_all_sticker_names() {
            Ok(names) => {
                let mut resp = ResponseBuilder::new();
                for name in names {
                    resp.field("name", &name);
                    resp.field("type", "song");
                }
                resp.ok()
            }
            Err(e) => ResponseBuilder::error(
                ACK_ERROR_SYS,
                0,
                "stickernamestypes",
                &format!("Error: {e}"),
            ),
        }
    })
    .await
    .unwrap_or_else(|_| {
        ResponseBuilder::error(ACK_ERROR_SYS, 0, "stickernamestypes", "internal error")
    })
}
