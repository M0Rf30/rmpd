// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Webhook notifier: POSTs every (selected) player event as JSON.
//!
//! ```toml
//! [[integration]]
//! name = "hook"
//! type = "webhook"
//! url = "https://example.org/rmpd-hook"
//! # events = ["song_changed", "player_state_changed"]   # default: all except noisy ones
//! # secret = "shared secret"                           # adds X-Rmpd-Signature: sha256=<hmac hex>
//! # signature_header = "X-Rmpd-Signature"
//! # headers = { Authorization = "Bearer abc" }
//! ```
//!
//! Body: `{"event": "<snake_case name>", "timestamp": <unix secs>, "payload": <event data>}`.
//! Delivery is asynchronous through a bounded queue; when it is full, events
//! are dropped rather than blocking the player.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rmpd_core::config::IntegrationConfig;
use rmpd_core::event::Event;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{Integration, IntegrationContext, IntegrationPlugin};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

/// Settings accepted by this integration.
pub const SETTINGS: &[&str] = &["url", "events", "headers", "secret", "signature_header"];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "webhook",
    settings: SETTINGS,
    factory,
};

const DEFAULT_SIGNATURE_HEADER: &str = "X-Rmpd-Signature";
const QUEUE_SIZE: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];

/// Events that fire many times per second; excluded unless requested
/// explicitly (or via `events = ["*"]`).
const NOISY: &[&str] = &[
    "position_changed",
    "bitrate_changed",
    "database_update_progress",
];

/// Which events get delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilter {
    /// `None`: everything except [`NOISY`]. `Some(set)`: exactly `set`.
    names: Option<HashSet<String>>,
    all: bool,
}

impl EventFilter {
    /// Default filter: every event except the noisy ones.
    #[must_use]
    pub fn default_set() -> Self {
        Self {
            names: None,
            all: false,
        }
    }

    /// Build from user-supplied names (`CamelCase` or `snake_case`);
    /// `"*"` / `"all"` selects literally everything.
    #[must_use]
    pub fn from_names<I: IntoIterator<Item = String>>(names: I) -> Self {
        let mut set = HashSet::new();
        let mut all = false;
        for n in names {
            let n = n.trim();
            if n == "*" || n.eq_ignore_ascii_case("all") {
                all = true;
            } else if !n.is_empty() {
                set.insert(snake_case(n));
            }
        }
        Self {
            names: Some(set),
            all,
        }
    }

    #[must_use]
    pub fn allows(&self, event: &str) -> bool {
        if self.all {
            return true;
        }
        match &self.names {
            None => !NOISY.contains(&event),
            Some(set) => set.contains(event),
        }
    }
}

/// `PlayerStateChanged` -> `player_state_changed` (idempotent on snake_case).
#[must_use]
pub fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Split a serialized [`Event`] into `(snake_case name, payload)`.
#[must_use]
pub fn event_parts(event: &Event) -> Option<(String, Value)> {
    match serde_json::to_value(event).ok()? {
        Value::String(name) => Some((snake_case(&name), Value::Null)),
        Value::Object(map) if map.len() == 1 => map
            .into_iter()
            .next()
            .map(|(name, payload)| (snake_case(&name), payload)),
        _ => None,
    }
}

/// Request body for an event.
#[must_use]
pub fn build_body(name: &str, payload: Value, timestamp: u64) -> String {
    json!({ "event": name, "timestamp": timestamp, "payload": payload }).to_string()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn sha256(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().iter().copied().collect()
}

/// HMAC-SHA256 (RFC 2104).
#[must_use]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK {
        sha256(&[key])
    } else {
        key.to_vec()
    };
    k.resize(BLOCK, 0);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let inner = sha256(&[&ipad, message]);
    sha256(&[&opad, &inner])
}

/// Value of the signature header: `sha256=<hex hmac of body>`.
#[must_use]
pub fn signature(secret: &str, body: &[u8]) -> String {
    format!("sha256={}", hex(&hmac_sha256(secret.as_bytes(), body)))
}

struct Webhook {
    name: String,
    url: String,
    filter: EventFilter,
    headers: HeaderMap,
    secret: Option<String>,
    signature_header: HeaderName,
}

fn parse_events(cfg: &IntegrationConfig) -> Result<EventFilter, PluginError> {
    match cfg.settings.get("events") {
        None => Ok(EventFilter::default_set()),
        Some(toml::Value::String(s)) => Ok(EventFilter::from_names([s.clone()])),
        Some(toml::Value::Array(a)) => {
            let mut names = Vec::new();
            for v in a {
                match v {
                    toml::Value::String(s) => names.push(s.clone()),
                    _ => {
                        return Err(PluginError::Config(
                            "webhook: `events` must be a list of strings".to_owned(),
                        ));
                    }
                }
            }
            Ok(EventFilter::from_names(names))
        }
        Some(_) => Err(PluginError::Config(
            "webhook: `events` must be a list of strings".to_owned(),
        )),
    }
}

fn parse_headers(cfg: &IntegrationConfig) -> Result<HeaderMap, PluginError> {
    let mut map = HeaderMap::new();
    match cfg.settings.get("headers") {
        None => {}
        Some(toml::Value::Table(t)) => {
            for (k, v) in t {
                let toml::Value::String(v) = v else {
                    return Err(PluginError::Config(format!(
                        "webhook: header `{k}` must be a string"
                    )));
                };
                let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| {
                    PluginError::Config(format!("webhook: invalid header name `{k}`"))
                })?;
                let mut value = HeaderValue::from_str(v).map_err(|_| {
                    PluginError::Config(format!("webhook: invalid value for header `{k}`"))
                })?;
                value.set_sensitive(true);
                map.insert(name, value);
            }
        }
        Some(_) => {
            return Err(PluginError::Config(
                "webhook: `headers` must be a table".to_owned(),
            ));
        }
    }
    Ok(map)
}

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    let url = cfg
        .setting_str("url")
        .ok_or_else(|| PluginError::Config("webhook: `url` is required".to_owned()))?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(PluginError::Config(
            "webhook: `url` must start with http:// or https://".to_owned(),
        ));
    }
    let sig_name = cfg
        .setting_str("signature_header")
        .unwrap_or_else(|| DEFAULT_SIGNATURE_HEADER.to_owned());
    let signature_header = HeaderName::from_bytes(sig_name.as_bytes())
        .map_err(|_| PluginError::Config("webhook: invalid `signature_header`".to_owned()))?;
    Ok(Box::new(Webhook {
        name: cfg.name.clone(),
        url,
        filter: parse_events(cfg)?,
        headers: parse_headers(cfg)?,
        secret: cfg.setting_str("secret"),
        signature_header,
    }))
}

struct Delivery {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    secret: Option<String>,
    signature_header: HeaderName,
}

impl Delivery {
    async fn send(&self, body: String) -> Result<(), String> {
        let mut req = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header("Content-Type", "application/json");
        if let Some(secret) = &self.secret {
            req = req.header(
                self.signature_header.clone(),
                signature(secret, body.as_bytes()),
            );
        }
        let resp = req
            .body(body)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", status.as_u16()))
        }
    }

    async fn deliver(&self, name: &str, body: String) {
        let mut last = String::new();
        for attempt in 0..=RETRY_DELAYS.len() {
            match self.send(body.clone()).await {
                Ok(()) => return,
                Err(e) => last = e,
            }
            if let Some(d) = RETRY_DELAYS.get(attempt) {
                tokio::time::sleep(*d).await;
            }
        }
        tracing::warn!(%name, "webhook delivery failed, dropping event: {last}");
    }
}

#[async_trait]
impl Integration for Webhook {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let this = *self;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("rmpd/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| {
                PluginError::Runtime(format!("cannot build HTTP client: {}", e.without_url()))
            })?;
        let delivery = Delivery {
            client,
            url: this.url,
            headers: this.headers,
            secret: this.secret,
            signature_header: this.signature_header,
        };
        let IntegrationContext {
            mut events,
            mut shutdown,
            ..
        } = ctx;
        let (tx, mut rx) = mpsc::channel::<(String, String)>(QUEUE_SIZE);
        let name = this.name;
        let filter = this.filter;

        let worker_name = name.clone();
        let worker = async move {
            while let Some((event, body)) = rx.recv().await {
                // Events are delivered in order, one at a time.
                tokio::select! {
                    () = delivery.deliver(&worker_name, body) => {}
                    () = tokio::time::sleep(Duration::from_secs(60)) => {
                        tracing::warn!(name = %worker_name, %event, "webhook delivery timed out");
                    }
                }
            }
        };

        let front = async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    ev = events.recv() => match ev {
                        Ok(ev) => {
                            let Some((event, payload)) = event_parts(&ev) else { continue };
                            if !filter.allows(&event) {
                                continue;
                            }
                            let ts = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs());
                            let body = build_body(&event, payload, ts);
                            if tx.try_send((event, body)).is_err() {
                                tracing::warn!(%name, "webhook queue full, dropping event");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(%name, skipped = n, "webhook missed events (lagged)");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        };

        tokio::join!(front, worker);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::state::PlayerState;
    use std::time::Duration as D;

    fn cfg(settings: toml::Table) -> IntegrationConfig {
        IntegrationConfig {
            name: "hook".to_owned(),
            integration_type: "webhook".to_owned(),
            enabled: true,
            settings,
        }
    }

    #[test]
    fn hmac_rfc4231_vectors() {
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Key longer than the block size is hashed first.
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn signature_header_value() {
        assert_eq!(
            signature("Jefe", b"what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(snake_case("PlayerStateChanged"), "player_state_changed");
        assert_eq!(snake_case("song_changed"), "song_changed");
        assert_eq!(snake_case("SongFinished"), "song_finished");
    }

    #[test]
    fn event_name_and_payload() {
        let (n, p) = event_parts(&Event::SongFinished).expect("unit");
        assert_eq!((n.as_str(), p), ("song_finished", Value::Null));
        let (n, p) = event_parts(&Event::VolumeChanged(42)).expect("newtype");
        assert_eq!((n.as_str(), p), ("volume_changed", json!(42)));
        let (n, p) = event_parts(&Event::PlayerStateChanged(PlayerState::Play)).expect("state");
        assert_eq!((n.as_str(), p), ("player_state_changed", json!("Play")));
        let (n, p) = event_parts(&Event::PlaybackError {
            message: "boom".to_owned(),
            output: false,
            generation: 3,
        })
        .expect("struct");
        assert_eq!(n, "playback_error");
        assert_eq!(p["message"], "boom");
        assert_eq!(p["generation"], 3);
        let (n, _) = event_parts(&Event::PositionChanged(D::from_secs(1))).expect("pos");
        assert_eq!(n, "position_changed");
    }

    #[test]
    fn body_shape() {
        let body = build_body("volume_changed", json!(7), 1_700_000_000);
        let v: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["event"], "volume_changed");
        assert_eq!(v["timestamp"], 1_700_000_000u64);
        assert_eq!(v["payload"], 7);
    }

    #[test]
    fn default_filter_skips_noisy_events() {
        let f = EventFilter::default_set();
        assert!(f.allows("song_changed"));
        assert!(f.allows("queue_changed"));
        assert!(!f.allows("position_changed"));
        assert!(!f.allows("bitrate_changed"));
    }

    #[test]
    fn explicit_filter_is_exact_and_accepts_camel_case() {
        let f = EventFilter::from_names(["SongChanged".to_owned(), "volume_changed".to_owned()]);
        assert!(f.allows("song_changed"));
        assert!(f.allows("volume_changed"));
        assert!(!f.allows("queue_changed"));
        assert!(!f.allows("position_changed"));
        let f = EventFilter::from_names(["position_changed".to_owned()]);
        assert!(f.allows("position_changed"));
    }

    #[test]
    fn star_selects_everything() {
        let f = EventFilter::from_names(["*".to_owned()]);
        assert!(f.allows("position_changed"));
        assert!(f.allows("anything"));
    }

    #[test]
    fn config_parsing() {
        assert!(matches!(
            factory(&cfg(toml::Table::new())),
            Err(PluginError::Config(_))
        ));

        let mut t = toml::Table::new();
        t.insert("url".to_owned(), "ftp://x".into());
        assert!(factory(&cfg(t)).is_err());

        let mut t = toml::Table::new();
        t.insert("url".to_owned(), "https://example.org/hook".into());
        t.insert(
            "events".to_owned(),
            toml::Value::Array(vec!["song_changed".into()]),
        );
        let mut h = toml::Table::new();
        h.insert("X-Token".to_owned(), "abc".into());
        t.insert("headers".to_owned(), toml::Value::Table(h));
        t.insert("secret".to_owned(), "s3cret".into());
        assert!(factory(&cfg(t.clone())).is_ok());

        t.insert("events".to_owned(), toml::Value::Integer(3));
        assert!(factory(&cfg(t.clone())).is_err());
        t.insert("events".to_owned(), toml::Value::Array(vec![]));
        let mut bad = toml::Table::new();
        bad.insert("bad header".to_owned(), "x".into());
        t.insert("headers".to_owned(), toml::Value::Table(bad));
        assert!(factory(&cfg(t)).is_err());
    }
}
