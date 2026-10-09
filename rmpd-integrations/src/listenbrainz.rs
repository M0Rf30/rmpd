// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! ListenBrainz scrobbler (also works with compatible servers via `api_url`).
//!
//! ```toml
//! [[integration]]
//! name = "listenbrainz"
//! type = "listenbrainz"
//! token = "<user token from https://listenbrainz.org/settings/>"
//! # api_url = "https://api.listenbrainz.org"
//! # scrobble_streams = false
//! ```

use crate::scrobble::tracker::{Listen, TrackMeta};
use crate::scrobble::{
    ScrobbleBackend, ScrobbleSettings, SubmitError, http_client, run_scrobbler, transport_error,
};
use async_trait::async_trait;
use rmpd_core::config::IntegrationConfig;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{Integration, IntegrationContext, IntegrationPlugin};
use serde_json::{Value, json};

/// Default API root.
pub const DEFAULT_API_URL: &str = "https://api.listenbrainz.org";

/// Settings accepted by this integration.
pub const SETTINGS: &[&str] = &["token", "api_url", "scrobble_streams", "max_queue"];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "listenbrainz",
    settings: SETTINGS,
    factory,
};

/// ListenBrainz accepts up to 1000 listens per request; stay well below.
const MAX_BATCH: usize = 50;

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    let token = cfg
        .setting_str("token")
        .ok_or_else(|| PluginError::Config("listenbrainz: `token` is required".to_owned()))?;
    let api_url = normalize_api_url(
        &cfg.setting_str("api_url")
            .unwrap_or_else(|| DEFAULT_API_URL.to_owned()),
    )?;
    Ok(Box::new(ListenBrainz {
        name: cfg.name.clone(),
        token,
        api_url,
        settings: ScrobbleSettings::from_config(cfg)?,
    }))
}

fn normalize_api_url(url: &str) -> Result<String, PluginError> {
    let url = url.trim().trim_end_matches('/');
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(PluginError::Config(
            "listenbrainz: `api_url` must start with http:// or https://".to_owned(),
        ));
    }
    Ok(url.to_owned())
}

struct ListenBrainz {
    name: String,
    token: String,
    api_url: String,
    settings: ScrobbleSettings,
}

#[async_trait]
impl Integration for ListenBrainz {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let this = *self;
        let backend = Backend {
            client: http_client()?,
            endpoint: format!("{}/1/submit-listens", this.api_url),
            auth: format!("Token {}", this.token),
        };
        run_scrobbler(this.name, backend, this.settings, ctx).await
    }
}

struct Backend {
    client: reqwest::Client,
    endpoint: String,
    /// `Authorization` header value (secret).
    auth: String,
}

impl Backend {
    async fn post(&self, body: &Value) -> Result<(), SubmitError> {
        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", &self.auth)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(transport_error)?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        Err(classify_status(status.as_u16()))
    }
}

/// Map an HTTP failure status to a retry decision.
fn classify_status(code: u16) -> SubmitError {
    match code {
        400 | 413 | 422 => SubmitError::Permanent(format!("HTTP {code}")),
        401 | 403 => SubmitError::Transient(format!("HTTP {code} (check the `token` setting)")),
        _ => SubmitError::Transient(format!("HTTP {code}")),
    }
}

#[async_trait]
impl ScrobbleBackend for Backend {
    fn max_batch(&self) -> usize {
        MAX_BATCH
    }

    async fn now_playing(&self, meta: &TrackMeta) -> Result<(), SubmitError> {
        self.post(&playing_now_payload(meta)).await
    }

    async fn submit(&self, listens: &[Listen]) -> Result<(), SubmitError> {
        self.post(&submit_payload(listens)).await
    }
}

/// `track_metadata` object for one track.
fn track_metadata(meta: &TrackMeta) -> Value {
    let mut info = serde_json::Map::new();
    info.insert("media_player".to_owned(), json!("rmpd"));
    info.insert(
        "media_player_version".to_owned(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    info.insert("submission_client".to_owned(), json!("rmpd"));
    info.insert(
        "submission_client_version".to_owned(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    if let Some(d) = meta.duration_secs {
        info.insert("duration".to_owned(), json!(d));
    }
    if let Some(n) = &meta.track_number {
        info.insert("tracknumber".to_owned(), json!(n));
    }
    if let Some(v) = &meta.recording_mbid {
        info.insert("recording_mbid".to_owned(), json!(v));
    }
    if let Some(v) = &meta.release_mbid {
        info.insert("release_mbid".to_owned(), json!(v));
    }
    if let Some(v) = &meta.release_group_mbid {
        info.insert("release_group_mbid".to_owned(), json!(v));
    }
    if let Some(v) = &meta.track_mbid {
        info.insert("track_mbid".to_owned(), json!(v));
    }
    if !meta.artist_mbids.is_empty() {
        info.insert("artist_mbids".to_owned(), json!(meta.artist_mbids));
    }

    let mut md = serde_json::Map::new();
    md.insert("artist_name".to_owned(), json!(meta.artist));
    md.insert("track_name".to_owned(), json!(meta.title));
    if let Some(album) = &meta.album {
        md.insert("release_name".to_owned(), json!(album));
    }
    md.insert("additional_info".to_owned(), Value::Object(info));
    Value::Object(md)
}

/// Body of a `playing_now` submission.
fn playing_now_payload(meta: &TrackMeta) -> Value {
    json!({
        "listen_type": "playing_now",
        "payload": [{ "track_metadata": track_metadata(meta) }],
    })
}

/// Body of a `single` (one listen) or `import` (several) submission.
fn submit_payload(listens: &[Listen]) -> Value {
    let kind = if listens.len() == 1 {
        "single"
    } else {
        "import"
    };
    let payload: Vec<Value> = listens
        .iter()
        .map(|l| {
            json!({
                "listened_at": l.listened_at,
                "track_metadata": track_metadata(&l.meta),
            })
        })
        .collect();
    json!({ "listen_type": kind, "payload": payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> TrackMeta {
        TrackMeta {
            artist: "Artist".to_owned(),
            title: "Title".to_owned(),
            album: Some("Album".to_owned()),
            duration_secs: Some(200),
            track_number: Some("3".to_owned()),
            recording_mbid: Some("9f8b3c0e-1111-4222-8333-444455556666".to_owned()),
            artist_mbids: vec!["aaaaaaaa-1111-4222-8333-444455556666".to_owned()],
            ..TrackMeta::default()
        }
    }

    #[test]
    fn playing_now_has_no_timestamp() {
        let v = playing_now_payload(&meta());
        assert_eq!(v["listen_type"], "playing_now");
        assert!(v["payload"][0].get("listened_at").is_none());
        let md = &v["payload"][0]["track_metadata"];
        assert_eq!(md["artist_name"], "Artist");
        assert_eq!(md["track_name"], "Title");
        assert_eq!(md["release_name"], "Album");
        assert_eq!(
            md["additional_info"]["recording_mbid"],
            "9f8b3c0e-1111-4222-8333-444455556666"
        );
        assert_eq!(md["additional_info"]["media_player"], "rmpd");
        assert!(md["additional_info"].get("release_mbid").is_none());
    }

    #[test]
    fn single_vs_import() {
        let l = |t: i64| Listen {
            meta: meta(),
            listened_at: t,
        };
        let one = submit_payload(&[l(10)]);
        assert_eq!(one["listen_type"], "single");
        assert_eq!(one["payload"][0]["listened_at"], 10);
        let many = submit_payload(&[l(10), l(20)]);
        assert_eq!(many["listen_type"], "import");
        assert_eq!(many["payload"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            many["payload"][1]["track_metadata"]["additional_info"]["artist_mbids"][0],
            "aaaaaaaa-1111-4222-8333-444455556666"
        );
    }

    #[test]
    fn status_classification() {
        assert!(matches!(classify_status(400), SubmitError::Permanent(_)));
        assert!(matches!(classify_status(401), SubmitError::Transient(_)));
        assert!(matches!(classify_status(429), SubmitError::Transient(_)));
        assert!(matches!(classify_status(503), SubmitError::Transient(_)));
    }

    #[test]
    fn api_url_normalisation() {
        assert_eq!(
            normalize_api_url(" https://lb.example.org/ ")
                .ok()
                .as_deref(),
            Some("https://lb.example.org")
        );
        assert!(normalize_api_url("ftp://x").is_err());
    }

    #[test]
    fn token_is_required() {
        let cfg = IntegrationConfig {
            name: "lb".to_owned(),
            integration_type: "listenbrainz".to_owned(),
            enabled: true,
            settings: toml::Table::new(),
        };
        assert!(matches!(factory(&cfg), Err(PluginError::Config(_))));
        let mut settings = toml::Table::new();
        settings.insert("token".to_owned(), toml::Value::String("abc".to_owned()));
        let cfg = IntegrationConfig { settings, ..cfg };
        assert!(factory(&cfg).is_ok());
    }
}
