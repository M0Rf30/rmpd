// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JSON-RPC 2.0 request handling with a Mopidy-compatible `core.*` subset,
//! mapped onto [`PlayerHandle`]. Transport-independent: the HTTP and
//! WebSocket handlers both feed request bodies to [`handle_body`].

use super::model::{
    history_json, ref_json, search_result_json, state_name, tl_track_json, track_json,
};
use rmpd_core::state::PlayerState;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::PlayerHandle;
use serde_json::{Value, json};
use std::time::Duration;

/// Invalid JSON was received.
pub const PARSE_ERROR: i64 = -32700;
/// The JSON sent is not a valid request object.
pub const INVALID_REQUEST: i64 = -32600;
/// The method does not exist.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid method parameters.
pub const INVALID_PARAMS: i64 = -32602;
/// The player rejected or failed the call.
pub const SERVER_ERROR: i64 = -32000;

/// Every method the dispatcher implements: `(name, description, params)`.
/// Drives `core.describe`.
pub const METHODS: &[(&str, &str, &[&str])] = &[
    ("core.describe", "List the available methods.", &[]),
    ("core.get_version", "Get the rmpd version.", &[]),
    (
        "core.get_uri_schemes",
        "List the supported URI schemes.",
        &[],
    ),
    (
        "core.playback.play",
        "Play the given tracklist track, or the current/first one.",
        &["tl_track", "tlid"],
    ),
    ("core.playback.pause", "Pause playback.", &[]),
    ("core.playback.resume", "Resume paused playback.", &[]),
    ("core.playback.stop", "Stop playback.", &[]),
    ("core.playback.next", "Play the next track.", &[]),
    ("core.playback.previous", "Play the previous track.", &[]),
    (
        "core.playback.seek",
        "Seek to a position in milliseconds.",
        &["time_position"],
    ),
    (
        "core.playback.get_state",
        "Get the playback state: playing, paused or stopped.",
        &[],
    ),
    (
        "core.playback.get_time_position",
        "Get the playback position in milliseconds.",
        &[],
    ),
    (
        "core.playback.get_current_track",
        "Get the current track, or null.",
        &[],
    ),
    (
        "core.playback.get_current_tl_track",
        "Get the current tracklist track, or null.",
        &[],
    ),
    (
        "core.playback.get_current_tlid",
        "Get the current tracklist id, or null.",
        &[],
    ),
    ("core.mixer.get_volume", "Get the volume (0-100).", &[]),
    (
        "core.mixer.set_volume",
        "Set the volume (0-100).",
        &["volume"],
    ),
    (
        "core.tracklist.get_length",
        "Get the number of tracks in the tracklist.",
        &[],
    ),
    (
        "core.tracklist.get_tl_tracks",
        "Get the tracklist as TlTracks.",
        &[],
    ),
    (
        "core.tracklist.get_tracks",
        "Get the tracklist as Tracks.",
        &[],
    ),
    (
        "core.tracklist.add",
        "Add URIs to the tracklist; returns the added TlTracks.",
        &["tracks", "at_position", "uris"],
    ),
    ("core.tracklist.clear", "Clear the tracklist.", &[]),
    (
        "core.tracklist.index",
        "Get the position of the current (or given) track.",
        &["tl_track", "tlid"],
    ),
    ("core.tracklist.get_random", "Get random mode.", &[]),
    ("core.tracklist.get_repeat", "Get repeat mode.", &[]),
    ("core.tracklist.get_single", "Get single mode.", &[]),
    ("core.tracklist.set_random", "Set random mode.", &["value"]),
    ("core.tracklist.set_repeat", "Set repeat mode.", &["value"]),
    ("core.tracklist.set_single", "Set single mode.", &["value"]),
    (
        "core.library.browse",
        "List a library directory (null is the root) as Refs.",
        &["uri"],
    ),
    (
        "core.library.search",
        "Case-insensitive substring search; returns SearchResults.",
        &["query", "uris", "exact"],
    ),
    (
        "core.history.get_history",
        "Get the recently played tracks as [timestamp_ms, Ref] pairs, newest first.",
        &[],
    ),
    (
        "core.history.get_length",
        "Get the number of remembered played tracks.",
        &[],
    ),
];

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    fn to_value(&self) -> Value {
        let mut v = json!({ "code": self.code, "message": self.message });
        if let (Some(data), Some(obj)) = (&self.data, v.as_object_mut()) {
            obj.insert("data".to_owned(), data.clone());
        }
        v
    }
}

impl From<PluginError> for RpcError {
    fn from(e: PluginError) -> Self {
        let kind = match &e {
            PluginError::Config(_) => "Config",
            PluginError::Unavailable(_) => "Unavailable",
            PluginError::Runtime(_) => "Runtime",
        };
        Self {
            code: SERVER_ERROR,
            message: "Unhandled exception".to_owned(),
            data: Some(json!({ "type": kind, "message": e.to_string() })),
        }
    }
}

/// Positional-or-named parameter access (`params` may be an array or an
/// object). JSON `null` counts as absent.
struct Params<'a>(Option<&'a Value>);

impl<'a> Params<'a> {
    fn get(&self, index: usize, name: &str) -> Option<&'a Value> {
        let value = match self.0? {
            Value::Object(map) => map.get(name),
            Value::Array(items) => items.get(index),
            _ => None,
        }?;
        if value.is_null() { None } else { Some(value) }
    }

    fn require(&self, index: usize, name: &str) -> Result<&'a Value, RpcError> {
        self.get(index, name)
            .ok_or_else(|| RpcError::invalid_params(format!("missing parameter `{name}`")))
    }

    fn bool(&self, index: usize, name: &str) -> Result<bool, RpcError> {
        self.require(index, name)?
            .as_bool()
            .ok_or_else(|| RpcError::invalid_params(format!("`{name}` must be a boolean")))
    }

    fn int(&self, index: usize, name: &str) -> Result<i64, RpcError> {
        let v = self.require(index, name)?;
        v.as_i64()
            // Lossy by design: JS clients may send whole numbers as floats.
            .or_else(|| v.as_f64().map(|f| f as i64))
            .ok_or_else(|| RpcError::invalid_params(format!("`{name}` must be a number")))
    }

    /// Track-list id from `tlid`, or from a `tl_track` object / bare number.
    fn tlid(&self) -> Result<Option<u32>, RpcError> {
        let value = self.get(1, "tlid").or_else(|| self.get(0, "tl_track"));
        let Some(value) = value else {
            return Ok(None);
        };
        let raw = match value {
            Value::Object(o) => o.get("tlid").and_then(Value::as_u64),
            other => other.as_u64(),
        };
        raw.and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| RpcError::invalid_params("invalid `tlid`"))
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn describe() -> Value {
    let mut map = serde_json::Map::new();
    for (name, description, params) in METHODS {
        let params: Vec<Value> = params.iter().map(|p| json!({ "name": p })).collect();
        map.insert(
            (*name).to_owned(),
            json!({ "description": description, "params": params }),
        );
    }
    Value::Object(map)
}

/// Flatten a Mopidy search query (`{"any": ["foo"], "artist": ["bar"]}`) into
/// the substring handed to the library's all-tags search.
fn query_text(query: &Value) -> Result<String, RpcError> {
    let mut words: Vec<&str> = Vec::new();
    match query {
        Value::String(s) => words.push(s),
        Value::Object(map) => {
            for value in map.values() {
                match value {
                    Value::String(s) => words.push(s),
                    Value::Array(items) => words.extend(items.iter().filter_map(Value::as_str)),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let text = words.join(" ");
    if text.trim().is_empty() {
        return Err(RpcError::invalid_params("`query` has no search terms"));
    }
    Ok(text)
}

/// Execute one method. `params` is the request's `params` member.
///
/// # Errors
/// [`METHOD_NOT_FOUND`], [`INVALID_PARAMS`], or [`SERVER_ERROR`] when the
/// player call fails.
pub async fn dispatch(
    player: &dyn PlayerHandle,
    method: &str,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    let p = Params(params);
    match method {
        "core.describe" => Ok(describe()),
        "core.get_version" => Ok(json!(format!("rmpd {}", env!("CARGO_PKG_VERSION")))),
        "core.get_uri_schemes" => Ok(json!(["file"])),
        _ => {
            if let Some(name) = method.strip_prefix("core.playback.") {
                playback(player, name, &p).await
            } else if let Some(name) = method.strip_prefix("core.mixer.") {
                mixer(player, name, &p).await
            } else if let Some(name) = method.strip_prefix("core.tracklist.") {
                tracklist(player, name, &p).await
            } else if let Some(name) = method.strip_prefix("core.library.") {
                library(player, name, &p).await
            } else if let Some(name) = method.strip_prefix("core.history.") {
                history(player, name).await
            } else {
                Err(not_found(method))
            }
        }
    }
}

fn not_found(method: &str) -> RpcError {
    RpcError::new(METHOD_NOT_FOUND, format!("Method not found: {method}"))
}

async fn playback(
    player: &dyn PlayerHandle,
    name: &str,
    p: &Params<'_>,
) -> Result<Value, RpcError> {
    match name {
        "play" => {
            match p.tlid()? {
                Some(id) => player.play_id(id).await?,
                None => player.play().await?,
            }
            Ok(Value::Null)
        }
        "pause" => player
            .pause()
            .await
            .map(|()| Value::Null)
            .map_err(Into::into),
        "resume" => {
            if player.status().await.state == PlayerState::Pause {
                player.toggle().await?;
            }
            Ok(Value::Null)
        }
        "stop" => player
            .stop()
            .await
            .map(|()| Value::Null)
            .map_err(Into::into),
        "next" => player
            .next()
            .await
            .map(|()| Value::Null)
            .map_err(Into::into),
        "previous" => player
            .previous()
            .await
            .map(|()| Value::Null)
            .map_err(Into::into),
        "seek" => {
            let ms = p.int(0, "time_position")?.max(0).unsigned_abs();
            player.seek(Duration::from_millis(ms)).await?;
            Ok(json!(true))
        }
        "get_state" => Ok(json!(state_name(player.status().await.state))),
        "get_time_position" => Ok(json!(player.position().await.map_or(0, millis))),
        "get_current_track" => Ok(player
            .status()
            .await
            .song
            .map_or(Value::Null, |song| track_json(&song))),
        "get_current_tl_track" => {
            let song = player.status().await.song;
            let tlid = player.current_song_id().await;
            Ok(match (tlid, song) {
                (Some(id), Some(song)) => tl_track_json(id, &song),
                _ => Value::Null,
            })
        }
        "get_current_tlid" => Ok(json!(player.current_song_id().await)),
        _ => Err(not_found(&format!("core.playback.{name}"))),
    }
}

async fn mixer(player: &dyn PlayerHandle, name: &str, p: &Params<'_>) -> Result<Value, RpcError> {
    match name {
        "get_volume" => Ok(json!(player.status().await.volume)),
        "set_volume" => {
            let volume = p.int(0, "volume")?.clamp(0, 100);
            player
                .set_volume(u8::try_from(volume).unwrap_or(100))
                .await?;
            Ok(json!(true))
        }
        _ => Err(not_found(&format!("core.mixer.{name}"))),
    }
}

async fn tracklist(
    player: &dyn PlayerHandle,
    name: &str,
    p: &Params<'_>,
) -> Result<Value, RpcError> {
    match name {
        "get_length" => Ok(json!(player.queue_len().await)),
        "get_tl_tracks" => Ok(Value::Array(
            player
                .queue_entries()
                .await
                .iter()
                .map(|e| tl_track_json(e.id, &e.song))
                .collect(),
        )),
        "get_tracks" => Ok(Value::Array(
            player
                .queue_entries()
                .await
                .iter()
                .map(|e| track_json(&e.song))
                .collect(),
        )),
        "add" => {
            let uris = add_uris_param(p)?;
            let position = match p.get(1, "at_position") {
                Some(v) => Some(
                    v.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| RpcError::invalid_params("invalid `at_position`"))?,
                ),
                None => None,
            };
            let added = player.add_uris(&uris, position).await?;
            Ok(Value::Array(
                added.iter().map(|e| tl_track_json(e.id, &e.song)).collect(),
            ))
        }
        "clear" => player
            .clear_queue()
            .await
            .map(|()| Value::Null)
            .map_err(Into::into),
        "index" => {
            let wanted = match p.tlid()? {
                Some(id) => Some(id),
                None => player.current_song_id().await,
            };
            let position = match wanted {
                Some(id) => player
                    .queue_entries()
                    .await
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.position),
                None => None,
            };
            Ok(position.map_or(Value::Null, |n| json!(n)))
        }
        "get_random" => Ok(json!(player.options().await.random)),
        "get_repeat" => Ok(json!(player.options().await.repeat)),
        "get_single" => Ok(json!(player.options().await.single)),
        "set_random" => {
            player.set_random(p.bool(0, "value")?).await?;
            Ok(Value::Null)
        }
        "set_repeat" => {
            player.set_repeat(p.bool(0, "value")?).await?;
            Ok(Value::Null)
        }
        "set_single" => {
            player.set_single(p.bool(0, "value")?).await?;
            Ok(Value::Null)
        }
        _ => Err(not_found(&format!("core.tracklist.{name}"))),
    }
}

async fn library(player: &dyn PlayerHandle, name: &str, p: &Params<'_>) -> Result<Value, RpcError> {
    match name {
        "browse" => {
            let uri = match p.get(0, "uri") {
                Some(Value::String(s)) => Some(s.as_str()),
                Some(_) => return Err(RpcError::invalid_params("`uri` must be a string")),
                None => None,
            };
            let entries = player.browse(uri).await?;
            Ok(Value::Array(entries.iter().map(ref_json).collect()))
        }
        "search" => {
            let text = query_text(p.require(0, "query")?)?;
            let songs = player.search(&text).await?;
            Ok(json!([search_result_json(&songs)]))
        }
        _ => Err(not_found(&format!("core.library.{name}"))),
    }
}

async fn history(player: &dyn PlayerHandle, name: &str) -> Result<Value, RpcError> {
    match name {
        "get_history" => Ok(history_json(&player.history().await)),
        "get_length" => Ok(json!(player.history_length().await)),
        _ => Err(not_found(&format!("core.history.{name}"))),
    }
}

/// `uris` (or the deprecated `tracks` list of objects with a `uri`).
fn add_uris_param(p: &Params<'_>) -> Result<Vec<String>, RpcError> {
    if let Some(v) = p.get(2, "uris") {
        let items = v
            .as_array()
            .ok_or_else(|| RpcError::invalid_params("`uris` must be a list of strings"))?;
        return items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| RpcError::invalid_params("`uris` must be a list of strings"))
            })
            .collect();
    }
    if let Some(v) = p.get(0, "tracks") {
        let items = v
            .as_array()
            .ok_or_else(|| RpcError::invalid_params("`tracks` must be a list of tracks"))?;
        return items
            .iter()
            .map(|i| {
                i.get("uri")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| RpcError::invalid_params("every track needs a `uri`"))
            })
            .collect();
    }
    Err(RpcError::invalid_params("missing parameter `uris`"))
}

fn success(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: &Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() })
}

/// Handle one request object. `None` for notifications (no `id`).
async fn handle_one(player: &dyn PlayerHandle, request: &Value) -> Option<Value> {
    let Some(obj) = request.as_object() else {
        return Some(failure(
            &Value::Null,
            &RpcError::new(INVALID_REQUEST, "Invalid Request"),
        ));
    };
    let id = obj.get("id");
    let id_valid = matches!(
        id,
        None | Some(Value::Null | Value::String(_) | Value::Number(_))
    );
    let reply_id = if id_valid {
        id.cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Some(failure(
            &reply_id,
            &RpcError::new(INVALID_REQUEST, "Invalid Request: missing method"),
        ));
    };
    if !id_valid {
        return Some(failure(
            &reply_id,
            &RpcError::new(INVALID_REQUEST, "Invalid Request: bad id"),
        ));
    }
    let params = obj.get("params");
    let result = match params {
        Some(Value::Object(_) | Value::Array(_) | Value::Null) | None => {
            dispatch(player, method, params).await
        }
        Some(_) => Err(RpcError::invalid_params(
            "`params` must be an array or an object",
        )),
    };
    // A request without `id` is a notification: executed, never answered.
    id?;
    Some(match result {
        Ok(value) => success(&reply_id, value),
        Err(e) => failure(&reply_id, &e),
    })
}

/// Handle a raw JSON-RPC body (single request or batch). Returns the
/// serialized response, or `None` when nothing is to be sent (notifications).
pub async fn handle_body(player: &dyn PlayerHandle, body: &str) -> Option<String> {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            return Some(
                failure(&Value::Null, &RpcError::new(PARSE_ERROR, "Parse error")).to_string(),
            );
        }
    };
    match parsed {
        Value::Array(items) => {
            if items.is_empty() {
                return Some(
                    failure(
                        &Value::Null,
                        &RpcError::new(INVALID_REQUEST, "Invalid Request: empty batch"),
                    )
                    .to_string(),
                );
            }
            let mut replies = Vec::new();
            for item in &items {
                if let Some(reply) = handle_one(player, item).await {
                    replies.push(reply);
                }
            }
            if replies.is_empty() {
                None
            } else {
                Some(Value::Array(replies).to_string())
            }
        }
        other => handle_one(player, &other).await.map(|v| v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_api::testutil::MockPlayer;
    use rmpd_core::history::HistoryEntry;

    async fn call(player: &MockPlayer, request: Value) -> Value {
        let reply = handle_body(player, &request.to_string()).await.unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    fn req(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })
    }

    #[tokio::test]
    async fn envelope_echoes_id_and_version() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.mixer.get_volume", json!([]))).await;
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], 7);
        assert_eq!(r["result"], 40);
        assert!(r.get("error").is_none());
    }

    #[tokio::test]
    async fn playback_controls_map_to_player() {
        let p = MockPlayer::with_queue();
        for m in ["play", "pause", "stop", "next", "previous"] {
            let r = call(&p, req(&format!("core.playback.{m}"), Value::Null)).await;
            assert!(r["result"].is_null(), "{m}: {r}");
            assert!(r.get("error").is_none(), "{m}: {r}");
        }
        assert_eq!(p.calls(), ["play", "pause", "stop", "next", "previous"]);
    }

    #[tokio::test]
    async fn play_with_tlid_uses_play_id() {
        let p = MockPlayer::with_queue();
        call(&p, req("core.playback.play", json!({ "tlid": 11 }))).await;
        call(&p, req("core.playback.play", json!([null, 10]))).await;
        call(
            &p,
            req(
                "core.playback.play",
                json!({ "tl_track": { "__model__": "TlTrack", "tlid": 10 } }),
            ),
        )
        .await;
        assert_eq!(p.calls(), ["play_id:11", "play_id:10", "play_id:10"]);
    }

    #[tokio::test]
    async fn resume_only_toggles_when_paused() {
        let p = MockPlayer::with_queue();
        call(&p, req("core.playback.resume", json!([]))).await;
        assert!(p.calls().is_empty());
        *p.state.lock() = PlayerState::Pause;
        call(&p, req("core.playback.resume", json!([]))).await;
        assert_eq!(p.calls(), ["toggle"]);
    }

    #[tokio::test]
    async fn seek_and_position_use_milliseconds() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.playback.seek", json!([61_500]))).await;
        assert_eq!(r["result"], true);
        call(
            &p,
            req("core.playback.seek", json!({ "time_position": -5 })),
        )
        .await;
        assert_eq!(p.calls(), ["seek:61500", "seek:0"]);
        let r = call(&p, req("core.playback.get_time_position", json!([]))).await;
        assert_eq!(r["result"], 2500);
    }

    #[tokio::test]
    async fn state_and_current_track() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.playback.get_state", json!([]))).await;
        assert_eq!(r["result"], "stopped");
        let r = call(&p, req("core.playback.get_current_track", json!([]))).await;
        assert_eq!(r["result"]["name"], "First");
        let r = call(&p, req("core.playback.get_current_tl_track", json!([]))).await;
        assert_eq!(r["result"]["tlid"], 10);
        assert_eq!(r["result"]["track"]["artists"][0]["name"], "Me");
        let r = call(&p, req("core.playback.get_current_tlid", json!([]))).await;
        assert_eq!(r["result"], 10);
        p.queue.lock().clear();
        let r = call(&p, req("core.playback.get_current_track", json!([]))).await;
        assert!(r["result"].is_null());
    }

    #[tokio::test]
    async fn volume_is_clamped() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.mixer.set_volume", json!([250]))).await;
        assert_eq!(r["result"], true);
        call(&p, req("core.mixer.set_volume", json!({ "volume": -3 }))).await;
        assert_eq!(p.calls(), ["set_volume:100", "set_volume:0"]);
        let bad = call(&p, req("core.mixer.set_volume", json!({}))).await;
        assert_eq!(bad["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tracklist_reads_and_options() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.tracklist.get_length", json!([]))).await;
        assert_eq!(r["result"], 2);
        let r = call(&p, req("core.tracklist.get_tl_tracks", json!([]))).await;
        assert_eq!(r["result"][1]["tlid"], 11);
        assert_eq!(r["result"][1]["track"]["uri"], "b.flac");
        let r = call(&p, req("core.tracklist.get_tracks", json!([]))).await;
        assert_eq!(r["result"][0]["__model__"], "Track");
        let r = call(&p, req("core.tracklist.index", json!({ "tlid": 11 }))).await;
        assert_eq!(r["result"], 1);
        call(&p, req("core.tracklist.set_random", json!([true]))).await;
        call(
            &p,
            req("core.tracklist.set_repeat", json!({ "value": true })),
        )
        .await;
        call(&p, req("core.tracklist.set_single", json!([true]))).await;
        assert_eq!(
            p.calls(),
            ["set_random:true", "set_repeat:true", "set_single:true"]
        );
        for (m, want) in [
            ("get_random", true),
            ("get_repeat", true),
            ("get_single", true),
        ] {
            let r = call(&p, req(&format!("core.tracklist.{m}"), json!([]))).await;
            assert_eq!(r["result"], want, "{m}");
        }
        let bad = call(&p, req("core.tracklist.set_random", json!(["yes"]))).await;
        assert_eq!(bad["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tracklist_add_and_clear() {
        let p = MockPlayer::with_queue();
        let r = call(
            &p,
            req(
                "core.tracklist.add",
                json!({ "uris": ["x.flac", "y.flac"], "at_position": 1 }),
            ),
        )
        .await;
        assert_eq!(r["result"][0]["tlid"], 100);
        assert_eq!(r["result"][1]["track"]["uri"], "y.flac");
        let r = call(
            &p,
            req(
                "core.tracklist.add",
                json!({ "tracks": [{ "__model__": "Track", "uri": "z.flac" }] }),
            ),
        )
        .await;
        assert_eq!(r["result"][0]["track"]["uri"], "z.flac");
        call(&p, req("core.tracklist.clear", json!([]))).await;
        assert_eq!(
            p.calls(),
            [
                "add_uris:x.flac,y.flac:Some(1)",
                "add_uris:z.flac:None",
                "clear"
            ]
        );
        let bad = call(&p, req("core.tracklist.add", json!({}))).await;
        assert_eq!(bad["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn library_browse_and_search() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.library.browse", json!([null]))).await;
        assert_eq!(r["result"][0]["type"], "directory");
        assert_eq!(r["result"][1]["type"], "track");
        call(&p, req("core.library.browse", json!({ "uri": "dir" }))).await;
        let r = call(
            &p,
            req(
                "core.library.search",
                json!({ "query": { "any": ["beat", "les"] } }),
            ),
        )
        .await;
        assert_eq!(r["result"][0]["__model__"], "SearchResult");
        assert_eq!(r["result"][0]["tracks"][0]["name"], "Found");
        assert_eq!(
            p.calls(),
            ["browse:None", "browse:Some(\"dir\")", "search:beat les"]
        );
        let bad = call(&p, req("core.library.search", json!({ "query": {} }))).await;
        assert_eq!(bad["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn history_get_history_and_length() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.history.get_length", json!([]))).await;
        assert_eq!(r["result"], 0);
        let r = call(&p, req("core.history.get_history", json!([]))).await;
        assert_eq!(r["result"], json!([]));

        // Newest first, as the player hands it over.
        *p.history.lock() = vec![
            HistoryEntry {
                timestamp_ms: 2_000,
                uri: "b.flac".into(),
                title: Some("Second".into()),
                artist: None,
                album: None,
            },
            HistoryEntry {
                timestamp_ms: 1_000,
                uri: "dir/a.flac".into(),
                title: None,
                artist: None,
                album: None,
            },
        ];
        let r = call(&p, req("core.history.get_length", json!([]))).await;
        assert_eq!(r["result"], 2);
        let r = call(&p, req("core.history.get_history", json!([]))).await;
        let items = r["result"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0], 2_000);
        assert_eq!(items[0][1]["__model__"], "Ref");
        assert_eq!(items[0][1]["type"], "track");
        assert_eq!(items[0][1]["uri"], "b.flac");
        assert_eq!(items[0][1]["name"], "Second");
        assert_eq!(items[1][0], 1_000);
        assert_eq!(items[1][1]["name"], "a.flac");
    }

    #[tokio::test]
    async fn describe_lists_every_method_and_all_dispatch() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.describe", json!([]))).await;
        let described = r["result"].as_object().unwrap();
        assert_eq!(described.len(), METHODS.len());
        assert!(described.contains_key("core.playback.play"));
        assert_eq!(
            described["core.playback.seek"]["params"][0]["name"],
            "time_position"
        );
        // Every advertised method is routed (never "method not found").
        for (name, _, _) in METHODS {
            let r = call(&p, req(name, json!({}))).await;
            assert_ne!(r["error"]["code"], METHOD_NOT_FOUND, "{name}");
        }
    }

    #[tokio::test]
    async fn unknown_method_and_bad_envelope() {
        let p = MockPlayer::with_queue();
        let r = call(&p, req("core.nope", json!([]))).await;
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = call(&p, req("core.playback.nope", json!([]))).await;
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = call(&p, json!({ "jsonrpc": "2.0", "id": 1 })).await;
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = call(&p, json!([])).await;
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = call(&p, req("core.get_version", json!("scalar"))).await;
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let raw = handle_body(&p, "{not json").await.unwrap();
        let r: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert!(r["id"].is_null());
    }

    #[tokio::test]
    async fn notifications_run_but_get_no_reply() {
        let p = MockPlayer::with_queue();
        let body = json!({ "jsonrpc": "2.0", "method": "core.playback.pause" });
        assert!(handle_body(&p, &body.to_string()).await.is_none());
        assert_eq!(p.calls(), ["pause"]);
    }

    #[tokio::test]
    async fn batch_returns_array_of_replies() {
        let p = MockPlayer::with_queue();
        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "core.mixer.get_volume" },
            { "jsonrpc": "2.0", "method": "core.playback.stop" },
            { "jsonrpc": "2.0", "id": "b", "method": "core.nope" },
        ]);
        let reply = handle_body(&p, &batch.to_string()).await.unwrap();
        let r: Value = serde_json::from_str(&reply).unwrap();
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["result"], 40);
        assert_eq!(arr[1]["id"], "b");
        assert_eq!(arr[1]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(p.calls(), ["stop"]);
    }

    #[tokio::test]
    async fn player_errors_become_server_errors() {
        struct Failing;
        #[async_trait::async_trait]
        impl PlayerHandle for Failing {
            async fn status(&self) -> rmpd_plugin::integration::PlayerSnapshot {
                rmpd_plugin::integration::PlayerSnapshot {
                    state: PlayerState::Stop,
                    elapsed: None,
                    duration: None,
                    volume: 0,
                    song: None,
                }
            }
            async fn play(&self) -> Result<(), PluginError> {
                Err(PluginError::Runtime("ACK boom".to_owned()))
            }
            async fn pause(&self) -> Result<(), PluginError> {
                Ok(())
            }
            async fn toggle(&self) -> Result<(), PluginError> {
                Ok(())
            }
            async fn next(&self) -> Result<(), PluginError> {
                Ok(())
            }
            async fn previous(&self) -> Result<(), PluginError> {
                Ok(())
            }
            async fn stop(&self) -> Result<(), PluginError> {
                Ok(())
            }
            async fn set_volume(&self, _volume: u8) -> Result<(), PluginError> {
                Ok(())
            }
            async fn seek(&self, _position: Duration) -> Result<(), PluginError> {
                Ok(())
            }
        }
        let body = req("core.playback.play", json!([])).to_string();
        let r: Value = serde_json::from_str(&handle_body(&Failing, &body).await.unwrap()).unwrap();
        assert_eq!(r["error"]["code"], SERVER_ERROR);
        assert!(
            r["error"]["data"]["message"]
                .as_str()
                .unwrap()
                .contains("boom")
        );
        // Optional capabilities default to "unavailable" rather than panicking.
        let body = req("core.tracklist.clear", json!([])).to_string();
        let r: Value = serde_json::from_str(&handle_body(&Failing, &body).await.unwrap()).unwrap();
        assert_eq!(r["error"]["data"]["type"], "Unavailable");
    }
}
