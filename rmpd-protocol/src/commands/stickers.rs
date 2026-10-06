// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sticker (metadata tag) command handlers
//!
//! Stickers are arbitrary key-value metadata tags that can be attached to
//! songs and, since MPD 0.24, to stored playlists, allowed tag values and
//! filter expressions. They are stored persistently in the database (keyed by
//! type, uri and name, like MPD's `sticker` table) and can be used for
//! ratings, playback counts, or any custom metadata.
//!
//! The domain (`TYPE` argument) decides what the URI means, mirroring the
//! per-domain handlers of `command/StickerCommands.cxx`:
//!
//! | TYPE            | URI                                   | validated against           |
//! |-----------------|---------------------------------------|-----------------------------|
//! | `song`          | file path in the database             | song must exist             |
//! | `playlist`      | stored playlist name                  | playlist must exist         |
//! | tag name        | tag value                             | tag must be allowed + exist |
//! | `filter`        | filter expression (normalized)        | must parse and match a song |
//!
//! `sticker find` never validates its URI: it is a (directory, for `song`)
//! prefix, and the empty string matches everything.

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_NO_EXIST, ACK_ERROR_SYS, ACK_ERROR_UNKNOWN, apply_range,
    internal_error, open_db, sys_error,
};

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

/// A resolved sticker domain (`handle_sticker`'s handler selection).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Domain {
    Song,
    Playlist,
    Filter,
    /// Canonical MPD tag name (`tag_item_names[]`), allowed or not: MPD picks
    /// the tag handler for any tag name and only rejects a disallowed one
    /// when it validates a URI.
    Tag(&'static str),
}

impl Domain {
    /// The `type` column value and the field name `sticker find` prints.
    fn type_name(&self) -> &str {
        match self {
            Domain::Song => "song",
            Domain::Playlist => "playlist",
            Domain::Filter => "filter",
            Domain::Tag(name) => name,
        }
    }
}

/// Resolve the `TYPE` argument like `handle_sticker`: `song`, `playlist` and
/// `filter` are exact; anything else must be a tag name (matched
/// case-insensitively, then normalized to its canonical spelling).
fn resolve_domain(sticker_type: &str, command: &str) -> Result<Domain, String> {
    match sticker_type {
        "song" => return Ok(Domain::Song),
        "playlist" => return Ok(Domain::Playlist),
        "filter" => return Ok(Domain::Filter),
        _ => {}
    }
    let canonical = rmpd_core::song::canonical_tag_name(&sticker_type.to_ascii_lowercase());
    if canonical != "Unknown" {
        return Ok(Domain::Tag(canonical));
    }
    Err(ResponseBuilder::error(
        ACK_ERROR_ARG,
        0,
        command,
        &format!("unknown sticker domain {sticker_type:?}"),
    ))
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

fn arg_error(msg: &str) -> String {
    ResponseBuilder::error(ACK_ERROR_ARG, 0, "sticker", msg)
}

/// Whether the stored playlist `name` exists (`PlaylistVector::exists`).
fn playlist_exists(state: &AppState, name: &str) -> Result<bool, String> {
    let Some(dir) = &state.playlist_dir else {
        return Err(ResponseBuilder::error(
            ACK_ERROR_NO_EXIST,
            0,
            "sticker",
            "Stored playlists are disabled",
        ));
    };
    if super::playlists::validate_playlist_name(name).is_err() {
        return Ok(false);
    }
    Ok(std::path::Path::new(dir)
        .join(format!("{name}.m3u"))
        .is_file())
}

/// Validate a command URI for `domain` and return the URI to key the sticker
/// on (`DomainHandler::ValidateUri` and its per-domain overrides).
fn validate_uri(
    state: &AppState,
    db: &rmpd_library::Database,
    domain: &Domain,
    uri: &str,
) -> Result<String, String> {
    match domain {
        Domain::Song => {
            require_song(db, uri)?;
            Ok(uri.to_string())
        }
        Domain::Playlist => {
            if playlist_exists(state, uri)? {
                Ok(uri.to_string())
            } else {
                Err(arg_error(&format!("no such playlist: {uri:?}")))
            }
        }
        Domain::Tag(name) => {
            if !STICKER_ALLOWED_TAGS.contains(name) {
                return Err(arg_error(&format!("unsupported tag: {name:?}")));
            }
            let expr = rmpd_core::filter::FilterExpression::Compare {
                tag: name.to_ascii_lowercase(),
                op: rmpd_core::filter::CompareOp::Equal,
                value: uri.to_string(),
                case_sensitive: true,
                negated: false,
            };
            match db.filter_matches_any(&expr) {
                Ok(true) => Ok(uri.to_string()),
                Ok(false) => Err(arg_error(&format!("no such {name}: {uri:?}"))),
                Err(e) => Err(sys_error("sticker", e)),
            }
        }
        Domain::Filter => {
            // `MakeSongFilter(uri)`: a non-`(` argument is the legacy
            // `TAG VALUE` form, which needs a second argument we never have.
            // MPD's parse failures are `std::runtime_error` (ACK_ERROR_UNKNOWN).
            let parse_error = |msg: &str| {
                ResponseBuilder::error(
                    ACK_ERROR_UNKNOWN,
                    0,
                    "sticker",
                    msg.strip_prefix("Parse error: ").unwrap_or(msg),
                )
            };
            if !uri.starts_with('(') {
                return Err(parse_error("Incorrect number of filter arguments"));
            }
            let expr = rmpd_core::filter::FilterExpression::parse(uri, false)
                .map_err(|e| parse_error(&e.to_string()))?;
            let normalized = expr.to_expression();
            match db.filter_matches_any(&expr) {
                Ok(true) => Ok(normalized),
                Ok(false) => Err(arg_error(&format!("no matches found: {normalized:?}"))),
                Err(e) => Err(sys_error("sticker", e)),
            }
        }
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
/// domain (`args[1]`) before ever checking the subcommand (StickerCommands.cxx
/// `handle_sticker`), so an invalid domain still reports "unknown sticker
/// domain" here; only a valid domain reaches the generic "bad request".
pub fn handle_sticker_invalid_command(sticker_type: &str) -> String {
    if let Err(e) = resolve_domain(sticker_type, "sticker") {
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
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
    let state = state.clone();
    let uri = uri.to_string();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "sticker") {
            Ok(d) => d,
            Err(e) => return e,
        };

        // MPD validates the URI before the sticker lookup.
        let key = match validate_uri(&state, &db, &domain, &uri) {
            Ok(k) => k,
            Err(e) => return e,
        };

        match db.get_sticker_typed(domain.type_name(), &key, &name) {
            // MPD's `LoadValue` returns an empty string for "absent", so an
            // empty stored value is also "no such sticker".
            Ok(Some(value)) if !value.is_empty() => {
                let mut resp = ResponseBuilder::new();
                resp.field("sticker", format!("{name}={value}"));
                resp.ok()
            }
            Ok(_) => ResponseBuilder::error(
                ACK_ERROR_NO_EXIST,
                0,
                "sticker",
                &format!("no such sticker: {:?}", name),
            ),
            Err(e) => sys_error("sticker", e),
        }
    })
    .await
    .unwrap_or_else(|_| internal_error("sticker"))
}

pub async fn handle_sticker_set_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: &str,
    value: &str,
) -> String {
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
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

        let key = match validate_uri(&state_owned, &db, &domain, &uri) {
            Ok(k) => k,
            Err(e) => return (false, e),
        };

        match db.set_sticker_typed(domain.type_name(), &key, &name, &value) {
            Ok(_) => (true, ResponseBuilder::new().ok()),
            Err(e) => (false, sys_error("sticker", e)),
        }
    })
    .await
    .unwrap_or_else(|_| (false, internal_error("sticker")));
    if changed {
        notify_sticker_changed(state);
    }
    response
}

/// `delete TYPE URI [NAME]`. Like `DomainHandler::Delete`, nothing removed is
/// an error: `no such sticker: "NAME"` for a named delete, otherwise
/// `no stickers found: "URI"` (both `ACK_ERROR_NO_EXIST`).
pub async fn handle_sticker_delete_command(
    state: &AppState,
    sticker_type: &str,
    uri: &str,
    name: Option<&str>,
) -> String {
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
    let state_owned = state.clone();
    let uri = uri.to_string();
    let name = name.map(|s| s.to_string());
    let (changed, response) = tokio::task::spawn_blocking(move || {
        let name = name.as_deref();
        let db = match open_db(&state_owned, "sticker") {
            Ok(d) => d,
            Err(e) => return (false, e),
        };

        let key = match validate_uri(&state_owned, &db, &domain, &uri) {
            Ok(k) => k,
            Err(e) => return (false, e),
        };

        match db.delete_sticker_typed(domain.type_name(), &key, name) {
            Ok(true) => (true, ResponseBuilder::new().ok()),
            Ok(false) => {
                let msg = match name {
                    Some(n) => format!("no such sticker: {n:?}"),
                    None => format!("no stickers found: {uri:?}"),
                };
                (
                    false,
                    ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "sticker", &msg),
                )
            }
            Err(e) => (false, sys_error("sticker", e)),
        }
    })
    .await
    .unwrap_or_else(|_| (false, internal_error("sticker")));
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
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
    let state = state.clone();
    let uri = uri.to_string();
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "sticker") {
            Ok(d) => d,
            Err(e) => return e,
        };

        let key = match validate_uri(&state, &db, &domain, &uri) {
            Ok(k) => k,
            Err(e) => return e,
        };

        match db.list_stickers_typed(domain.type_name(), &key) {
            Ok(stickers) => {
                let mut resp = ResponseBuilder::new();
                for (name, value) in stickers {
                    resp.field("sticker", format!("{name}={value}"));
                }
                resp.ok()
            }
            Err(e) => sys_error("sticker", e),
        }
    })
    .await
    .unwrap_or_else(|_| internal_error("sticker"))
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
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

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

        match db.find_stickers_typed(domain.type_name(), &uri, &name) {
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

                // MPD prints `file: URI` for songs and `TYPE: URI` (the
                // canonical type name) for every other domain.
                let uri_field = match domain {
                    Domain::Song => "file",
                    _ => domain.type_name(),
                };
                let mut resp = ResponseBuilder::new();
                for (found_uri, sticker_value) in results {
                    resp.field(uri_field, found_uri);
                    resp.field("sticker", format!("{name}={sticker_value}"));
                }
                resp.ok()
            }
            Err(e) => sys_error("sticker", e),
        }
    })
    .await
    .unwrap_or_else(|_| internal_error("sticker"))
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
    let domain = match resolve_domain(sticker_type, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
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
        let key = match validate_uri(&state_owned, &db, &domain, &uri) {
            Ok(k) => k,
            Err(e) => return (false, e),
        };
        match db.adjust_sticker_typed(domain.type_name(), &key, &name, i64::from(delta)) {
            // MPD's Inc/Dec (StickerCommands.cxx) never print the new
            // value: just OK, unlike Get/Find's `sticker_print_value`.
            Ok(_) => (true, ResponseBuilder::new().ok()),
            Err(e) => (false, sys_error("sticker", e)),
        }
    })
    .await
    .unwrap_or_else(|_| (false, internal_error("sticker")));
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

/// Remove every sticker attached to the stored playlist `name`, mirroring
/// `Instance::OnPlaylistDeleted` (called by `rm`). Best effort like MPD:
/// failures are ignored; idle `sticker` clients are notified if any row went.
pub fn delete_playlist_stickers(state: &AppState, name: &str) {
    let Ok(db) = open_db(state, "rm") else {
        return;
    };
    if matches!(db.delete_sticker_typed("playlist", name, None), Ok(true)) {
        notify_sticker_changed(state);
    }
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
            Err(e) => sys_error("stickernames", e),
        }
    })
    .await
    .unwrap_or_else(|_| internal_error("stickernames"))
}

/// List available sticker types, matching MPD's `handle_sticker_types`
/// (StickerCommands.cxx:504) byte for byte: `filter`, `playlist`, `song`,
/// then every tag in `sticker_allowed_tags`.
pub async fn handle_sticker_types_command() -> String {
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
/// Mirrors MPD's `handle_sticker_names_types` (StickerCommands.cxx): `song`,
/// `playlist`, `filter`, and any tag in `sticker_allowed_tags` are valid
/// TYPEs and filter the listing (a domain with no stickers yields a bare
/// `OK`). Only a TYPE that is not a tag name at all (`no such tag`) or a tag
/// outside the allowed set (`unsupported tag`) is an error. Unlike the
/// `sticker` command, MPD matches the tag name case-sensitively here.
pub async fn handle_sticker_namestypes_command(
    state: &AppState,
    sticker_type: Option<&str>,
) -> String {
    if let Some(t) = sticker_type
        && !matches!(t, "song" | "playlist" | "filter")
        && !STICKER_ALLOWED_TAGS.contains(&t)
    {
        // MPD uses the case-sensitive tag_name_parse() here, unlike the
        // `sticker` command's case-insensitive tag_name_parse_i().
        let canonical = rmpd_core::song::canonical_tag_name(&t.to_ascii_lowercase());
        let known_tag = canonical != "Unknown" && canonical == t;
        let msg = if known_tag {
            format!("unsupported tag {t:?}")
        } else {
            format!("no such tag {t:?}")
        };
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "stickernamestypes", &msg);
    }
    let state = state.clone();
    let sticker_type = sticker_type.map(|s| s.to_string());
    tokio::task::spawn_blocking(move || {
        let db = match open_db(&state, "stickernamestypes") {
            Ok(d) => d,
            Err(e) => return e,
        };
        match db.list_sticker_names_types(sticker_type.as_deref()) {
            Ok(pairs) => {
                let mut resp = ResponseBuilder::new();
                for (name, ty) in pairs {
                    resp.field("name", &name);
                    resp.field("type", &ty);
                }
                resp.ok()
            }
            Err(e) => sys_error("stickernamestypes", e),
        }
    })
    .await
    .unwrap_or_else(|_| internal_error("stickernamestypes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_domain_is_exact_for_builtin_types_and_nocase_for_tags() {
        assert_eq!(resolve_domain("song", "sticker"), Ok(Domain::Song));
        assert_eq!(resolve_domain("playlist", "sticker"), Ok(Domain::Playlist));
        assert_eq!(resolve_domain("filter", "sticker"), Ok(Domain::Filter));
        assert_eq!(resolve_domain("album", "sticker"), Ok(Domain::Tag("Album")));
        assert_eq!(
            resolve_domain("musicbrainz_albumid", "sticker"),
            Ok(Domain::Tag("MUSICBRAINZ_ALBUMID"))
        );
        // Valid tag, but not an allowed sticker tag: still the tag handler
        // (rejected later, at URI validation time).
        assert_eq!(resolve_domain("TRACK", "sticker"), Ok(Domain::Tag("Track")));
        // `song` is not matched case-insensitively.
        let err = resolve_domain("Song", "sticker").unwrap_err();
        assert!(err.contains("unknown sticker domain \"Song\""), "{err}");
        assert!(resolve_domain("bogus", "sticker").is_err());
    }
}
