//! Sticker (metadata tag) command handlers
//!
//! Stickers are arbitrary key-value metadata tags that can be attached to songs.
//! They are stored persistently in the database and can be used for ratings,
//! playback counts, or any custom metadata.

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{ACK_ERROR_NO_EXIST, ACK_ERROR_SYS, open_db};

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

/// Comparison operator for `sticker find` filters.
#[derive(Clone, Copy)]
enum StickerCmp {
    Eq,
    Ne,
    Lt,
    Gt,
    Contains,
}

/// Decode a `value` field encoded by the parser as `"op\x00val"`.
/// Returns `None` when no operator filter is present.
fn decode_sticker_filter(encoded: Option<&str>) -> Option<(StickerCmp, &str)> {
    let enc = encoded?;
    let sep = enc.find('\x00')?;
    let op = match &enc[..sep] {
        "eq" => StickerCmp::Eq,
        "ne" => StickerCmp::Ne,
        "lt" => StickerCmp::Lt,
        "gt" => StickerCmp::Gt,
        "contains" => StickerCmp::Contains,
        _ => return None,
    };
    Some((op, &enc[sep + 1..]))
}

/// Test whether `sticker_value` satisfies `op cmp_value`.
/// `lt`/`gt` attempt numeric comparison first, then fall back to lexicographic.
fn sticker_matches(op: StickerCmp, sticker_value: &str, cmp_value: &str) -> bool {
    match op {
        StickerCmp::Eq => sticker_value == cmp_value,
        StickerCmp::Ne => sticker_value != cmp_value,
        StickerCmp::Contains => sticker_value.contains(cmp_value),
        StickerCmp::Lt => {
            if let (Ok(a), Ok(b)) = (sticker_value.parse::<f64>(), cmp_value.parse::<f64>()) {
                a < b
            } else {
                sticker_value < cmp_value
            }
        }
        StickerCmp::Gt => {
            if let (Ok(a), Ok(b)) = (sticker_value.parse::<f64>(), cmp_value.parse::<f64>()) {
                a > b
            } else {
                sticker_value > cmp_value
            }
        }
    }
}

pub async fn handle_sticker_get_command(state: &AppState, uri: &str, name: &str) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Check song exists (MPD validates URI before sticker lookup)
    if let Err(e) = require_song(&db, uri) {
        return e;
    }

    match db.get_sticker(uri, name) {
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
}

pub async fn handle_sticker_set_command(
    state: &AppState,
    uri: &str,
    name: &str,
    value: &str,
) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Check song exists
    if let Err(e) = require_song(&db, uri) {
        return e;
    }

    match db.set_sticker(uri, name, value) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_delete_command(
    state: &AppState,
    uri: &str,
    name: Option<&str>,
) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Check song exists
    if let Err(e) = require_song(&db, uri) {
        return e;
    }

    // When deleting a named sticker, check it exists first (MPD returns error if not found)
    if let Some(sticker_name) = name {
        match db.get_sticker(uri, sticker_name) {
            Ok(None) => {
                return ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    "sticker",
                    &format!("no such sticker: {:?}", sticker_name),
                );
            }
            Err(e) => {
                return ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}"));
            }
            Ok(Some(_)) => {}
        }
    }

    match db.delete_sticker(uri, name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_list_command(state: &AppState, uri: &str) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

    // Check song exists
    if let Err(e) = require_song(&db, uri) {
        return e;
    }

    match db.list_stickers(uri) {
        Ok(stickers) => {
            let mut resp = ResponseBuilder::new();
            for (name, value) in stickers {
                resp.field("sticker", format!("{name}={value}"));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_find_command(
    state: &AppState,
    uri: &str,
    name: &str,
    value: Option<&str>,
) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let filter = decode_sticker_filter(value);

    match db.find_stickers(uri, name) {
        Ok(results) => {
            let mut resp = ResponseBuilder::new();
            for (file_uri, sticker_value) in &results {
                if let Some((op, cmp_val)) = filter
                    && !sticker_matches(op, sticker_value, cmp_val)
                {
                    continue;
                }
                resp.field("file", file_uri);
                resp.field("sticker", format!("{name}={sticker_value}"));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
    }
}

/// Shared core for `sticker inc` / `sticker dec`.
/// `delta` is the signed change to apply (positive for inc, negative for dec).
async fn adjust_sticker_value(state: &AppState, uri: &str, name: &str, delta: i32) -> String {
    let db = match open_db(state, "sticker") {
        Ok(d) => d,
        Err(e) => return e,
    };
    let new_value = get_sticker_i32(&db, uri, name) + delta;
    match db.set_sticker(uri, name, &new_value.to_string()) {
        Ok(_) => {
            let mut resp = ResponseBuilder::new();
            resp.field("sticker", format!("{name}={new_value}"));
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "sticker", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_inc_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    adjust_sticker_value(state, uri, name, delta.unwrap_or(1)).await
}

pub async fn handle_sticker_dec_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    adjust_sticker_value(state, uri, name, -delta.unwrap_or(1)).await
}

pub async fn handle_sticker_names_command(state: &AppState, uri: Option<&str>) -> String {
    // List unique sticker names (optionally for specific URI)
    if let Some(uri_str) = uri {
        let db = match open_db(state, "stickernames") {
            Ok(d) => d,
            Err(e) => return e,
        };
        if let Ok(stickers) = db.list_stickers(uri_str) {
            let mut resp = ResponseBuilder::new();
            for (name, _) in stickers {
                resp.field("sticker", &name);
            }
            return resp.ok();
        }
    }
    ResponseBuilder::new().ok()
}

pub async fn handle_sticker_types_command() -> String {
    // List available sticker types, matching MPD's handle_sticker_types output order.
    // MPD outputs: filter, playlist, song, then sticker_allowed_tags intersected with tag_mask.
    let mut resp = ResponseBuilder::new();
    resp.field("stickertype", "filter");
    resp.field("stickertype", "playlist");
    resp.field("stickertype", "song");
    // Sticker-allowed tags (from MPD's AllowedTags.cxx), in enum order:
    for tag in &[
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
    ] {
        resp.field("stickertype", *tag);
    }
    resp.ok()
}

pub async fn handle_sticker_namestypes_command(state: &AppState, uri: Option<&str>) -> String {
    // List sticker names and types
    if let Some(uri_str) = uri {
        let db = match open_db(state, "stickernamestypes") {
            Ok(d) => d,
            Err(e) => return e,
        };
        if let Ok(stickers) = db.list_stickers(uri_str) {
            let mut resp = ResponseBuilder::new();
            for (name, _) in stickers {
                resp.field("sticker", format!("{name} song"));
            }
            return resp.ok();
        }
    }
    ResponseBuilder::new().ok()
}
