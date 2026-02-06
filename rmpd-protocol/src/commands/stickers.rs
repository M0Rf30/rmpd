//! Sticker (metadata tag) command handlers
//!
//! Stickers are arbitrary key-value metadata tags that can be attached to songs.
//! They are stored persistently in the database and can be used for ratings,
//! playback counts, or any custom metadata.

use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{open_db, ACK_ERROR_SYSTEM};

pub async fn handle_sticker_get_command(state: &AppState, uri: &str, name: &str) -> String {
    let db = match open_db(state, "sticker get") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.get_sticker(uri, name) {
        Ok(Some(value)) => {
            let mut resp = ResponseBuilder::new();
            resp.field("sticker", format!("{name}={value}"));
            resp.ok()
        }
        Ok(None) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker get", "no such sticker"),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker get", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_set_command(
    state: &AppState,
    uri: &str,
    name: &str,
    value: &str,
) -> String {
    let db = match open_db(state, "sticker set") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.set_sticker(uri, name, value) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker set", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_delete_command(
    state: &AppState,
    uri: &str,
    name: Option<&str>,
) -> String {
    let db = match open_db(state, "sticker delete") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.delete_sticker(uri, name) {
        Ok(_) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker delete", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_list_command(state: &AppState, uri: &str) -> String {
    let db = match open_db(state, "sticker list") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.list_stickers(uri) {
        Ok(stickers) => {
            let mut resp = ResponseBuilder::new();
            for (name, value) in stickers {
                resp.field("sticker", format!("{name}={value}"));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker list", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_find_command(
    state: &AppState,
    uri: &str,
    name: &str,
    _value: Option<&str>,
) -> String {
    let db = match open_db(state, "sticker find") {
        Ok(d) => d,
        Err(e) => return e,
    };

    match db.find_stickers(uri, name) {
        Ok(results) => {
            let mut resp = ResponseBuilder::new();
            for (file_uri, sticker_value) in results {
                resp.field("file", file_uri);
                resp.field("sticker", format!("{name}={sticker_value}"));
            }
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker find", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_inc_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    // Increment numeric sticker value
    let db = match open_db(state, "sticker inc") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let increment = delta.unwrap_or(1);
    let current = if let Ok(Some(val)) = db.get_sticker(uri, name) {
        val.parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    let new_value = current + increment;
    match db.set_sticker(uri, name, &new_value.to_string()) {
        Ok(_) => {
            let mut resp = ResponseBuilder::new();
            resp.field("sticker", format!("{name}={new_value}"));
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker inc", &format!("Error: {e}")),
    }
}

pub async fn handle_sticker_dec_command(
    state: &AppState,
    uri: &str,
    name: &str,
    delta: Option<i32>,
) -> String {
    // Decrement numeric sticker value
    let db = match open_db(state, "sticker dec") {
        Ok(d) => d,
        Err(e) => return e,
    };

    let decrement = delta.unwrap_or(1);
    let current = if let Ok(Some(val)) = db.get_sticker(uri, name) {
        val.parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    let new_value = current - decrement;
    match db.set_sticker(uri, name, &new_value.to_string()) {
        Ok(_) => {
            let mut resp = ResponseBuilder::new();
            resp.field("sticker", format!("{name}={new_value}"));
            resp.ok()
        }
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "sticker dec", &format!("Error: {e}")),
    }
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
    // List available sticker types (song is the primary type)
    let mut resp = ResponseBuilder::new();
    resp.field("sticker", "song");
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
