// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

// Last.fm's api_sig is computed over parameters sorted by name.
#![allow(clippy::disallowed_types)]

//! Last.fm scrobbler (also libre.fm / GNU FM via `api_url`).
//!
//! ```toml
//! [[integration]]
//! name = "lastfm"
//! type = "lastfm"
//! api_key = "<API key>"
//! api_secret = "<shared secret>"
//! session_key = "<session key, see below>"
//! # api_url = "https://ws.audioscrobbler.com/2.0/"   # libre.fm: https://libre.fm/2.0/
//! # scrobble_streams = false
//! ```
//!
//! # Obtaining a session key (web authentication flow)
//!
//! 1. Create an API account at <https://www.last.fm/api/account/create> to get
//!    an `api_key` and `api_secret`.
//! 2. Request a token (replace `KEY`):
//!    `https://ws.audioscrobbler.com/2.0/?method=auth.getToken&api_key=KEY&format=json`
//! 3. Authorise it in a browser: `https://www.last.fm/api/auth/?api_key=KEY&token=TOKEN`
//! 4. Compute `api_sig = md5("api_keyKEYmethodauth.getSessiontokenTOKENSECRET")`
//!    (`echo -n 'api_keyKEYmethodauth.getSessiontokenTOKENSECRET' | md5sum`) and open
//!    `https://ws.audioscrobbler.com/2.0/?method=auth.getSession&api_key=KEY&token=TOKEN&api_sig=SIG&format=json`
//! 5. The `session.key` in the answer never expires: use it as `session_key`.

use crate::scrobble::tracker::{Listen, TrackMeta};
use crate::scrobble::{
    ScrobbleBackend, ScrobbleSettings, SubmitError, http_client, run_scrobbler, transport_error,
};
use async_trait::async_trait;
use md5::{Digest, Md5};
use rmpd_core::config::IntegrationConfig;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{Integration, IntegrationContext, IntegrationPlugin};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Default API root (libre.fm: `https://libre.fm/2.0/`).
pub const DEFAULT_API_URL: &str = "https://ws.audioscrobbler.com/2.0/";

/// Settings accepted by this integration.
pub const SETTINGS: &[&str] = &[
    "api_key",
    "api_secret",
    "session_key",
    "api_url",
    "scrobble_streams",
    "max_queue",
];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "lastfm",
    settings: SETTINGS,
    factory,
};

/// `track.scrobble` accepts at most 50 tracks per call.
const MAX_BATCH: usize = 50;

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    let need = |key: &str| {
        cfg.setting_str(key)
            .ok_or_else(|| PluginError::Config(format!("lastfm: `{key}` is required")))
    };
    let api_url = cfg
        .setting_str("api_url")
        .unwrap_or_else(|| DEFAULT_API_URL.to_owned());
    if !(api_url.starts_with("https://") || api_url.starts_with("http://")) {
        return Err(PluginError::Config(
            "lastfm: `api_url` must start with http:// or https://".to_owned(),
        ));
    }
    Ok(Box::new(LastFm {
        name: cfg.name.clone(),
        api_key: need("api_key")?,
        api_secret: need("api_secret")?,
        session_key: need("session_key")?,
        api_url,
        settings: ScrobbleSettings::from_config(cfg)?,
    }))
}

struct LastFm {
    name: String,
    api_key: String,
    api_secret: String,
    session_key: String,
    api_url: String,
    settings: ScrobbleSettings,
}

#[async_trait]
impl Integration for LastFm {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let this = *self;
        let backend = Backend {
            client: http_client()?,
            api_url: this.api_url,
            api_key: this.api_key,
            api_secret: this.api_secret,
            session_key: this.session_key,
        };
        run_scrobbler(this.name, backend, this.settings, ctx).await
    }
}

struct Backend {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    api_secret: String,
    session_key: String,
}

impl Backend {
    /// Perform a signed POST call.
    async fn call(
        &self,
        method: &str,
        mut params: BTreeMap<String, String>,
    ) -> Result<(), SubmitError> {
        params.insert("method".to_owned(), method.to_owned());
        params.insert("api_key".to_owned(), self.api_key.clone());
        params.insert("sk".to_owned(), self.session_key.clone());
        let sig = api_sig(&params, &self.api_secret);
        params.insert("api_sig".to_owned(), sig);
        params.insert("format".to_owned(), "json".to_owned());

        let resp = self
            .client
            .post(&self.api_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_encode(&params))
            .send()
            .await
            .map_err(transport_error)?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<Value>(&text)
            && let Some(code) = v.get("error").and_then(Value::as_i64)
        {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
            return Err(classify_error(code, msg));
        }
        if status.is_success() {
            Ok(())
        } else {
            Err(SubmitError::Transient(format!("HTTP {}", status.as_u16())))
        }
    }
}

#[async_trait]
impl ScrobbleBackend for Backend {
    fn max_batch(&self) -> usize {
        MAX_BATCH
    }

    async fn now_playing(&self, meta: &TrackMeta) -> Result<(), SubmitError> {
        let mut params = BTreeMap::new();
        track_params(&mut params, meta, None, None);
        self.call("track.updateNowPlaying", params).await
    }

    async fn submit(&self, listens: &[Listen]) -> Result<(), SubmitError> {
        let mut params = BTreeMap::new();
        for (i, l) in listens.iter().enumerate() {
            track_params(&mut params, &l.meta, Some(i), Some(l.listened_at));
        }
        self.call("track.scrobble", params).await
    }
}

/// Add the parameters for one track; `index` selects the `key[i]` batch form.
fn track_params(
    params: &mut BTreeMap<String, String>,
    meta: &TrackMeta,
    index: Option<usize>,
    timestamp: Option<i64>,
) {
    let key = |name: &str| match index {
        Some(i) => format!("{name}[{i}]"),
        None => name.to_owned(),
    };
    params.insert(key("artist"), meta.artist.clone());
    params.insert(key("track"), meta.title.clone());
    if let Some(ts) = timestamp {
        params.insert(key("timestamp"), ts.to_string());
    }
    if let Some(v) = &meta.album {
        params.insert(key("album"), v.clone());
    }
    if let Some(v) = &meta.album_artist {
        params.insert(key("albumArtist"), v.clone());
    }
    if let Some(v) = &meta.track_number {
        params.insert(key("trackNumber"), v.clone());
    }
    if let Some(v) = &meta.recording_mbid {
        params.insert(key("mbid"), v.clone());
    }
    if let Some(d) = meta.duration_secs {
        params.insert(key("duration"), d.to_string());
    }
}

fn classify_error(code: i64, message: &str) -> SubmitError {
    let text = format!("Last.fm error {code}: {message}");
    match code {
        // Invalid parameters: retrying the same payload cannot succeed.
        6 => SubmitError::Permanent(text),
        // Everything else (auth, rate limit, outage) may recover.
        _ => SubmitError::Transient(text),
    }
}

/// Last.fm API signature: md5 over the concatenated `name+value` of all
/// parameters (sorted by name, excluding `format` and `callback`) followed by
/// the shared secret.
#[must_use]
pub fn api_sig(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut hasher = Md5::new();
    for (k, v) in params {
        if k == "format" || k == "callback" {
            continue;
        }
        hasher.update(k.as_bytes());
        hasher.update(v.as_bytes());
    }
    hasher.update(secret.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(32), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `application/x-www-form-urlencoded` body.
fn form_encode(params: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in params {
        if !out.is_empty() {
            out.push('&');
        }
        percent_encode_into(&mut out, k);
        out.push('=');
        percent_encode_into(&mut out, v);
    }
    out
}

fn percent_encode_into(out: &mut String, s: &str) {
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn signature_matches_reference() {
        // md5("api_keykeymethodauth.getSessiontokentoksecret")
        let p = map(&[
            ("method", "auth.getSession"),
            ("api_key", "key"),
            ("token", "tok"),
            ("format", "json"), // excluded from the signature
        ]);
        assert_eq!(api_sig(&p, "secret"), "04e870be4bb79756721b7bc1937fe83d");
    }

    #[test]
    fn signature_uses_raw_utf8_and_sorted_batch_keys() {
        let mut p = map(&[
            ("method", "track.scrobble"),
            ("api_key", "key"),
            ("sk", "sess"),
        ]);
        let meta = TrackMeta {
            artist: "Beyoncé & Coé".to_owned(),
            title: "Halo".to_owned(),
            album: Some("Album".to_owned()),
            ..TrackMeta::default()
        };
        track_params(&mut p, &meta, Some(0), Some(1_700_000_000));
        assert_eq!(api_sig(&p, "shh"), "0442d202cd829c82d16339b60b726d41");
    }

    #[test]
    fn track_params_single_and_batch_forms() {
        let meta = TrackMeta {
            artist: "A".to_owned(),
            title: "T".to_owned(),
            album_artist: Some("AA".to_owned()),
            track_number: Some("2".to_owned()),
            duration_secs: Some(180),
            recording_mbid: Some("m".to_owned()),
            ..TrackMeta::default()
        };
        let mut p = BTreeMap::new();
        track_params(&mut p, &meta, None, None);
        assert_eq!(p.get("artist").map(String::as_str), Some("A"));
        assert_eq!(p.get("albumArtist").map(String::as_str), Some("AA"));
        assert!(!p.contains_key("timestamp"));
        let mut p = BTreeMap::new();
        track_params(&mut p, &meta, Some(3), Some(42));
        assert_eq!(p.get("timestamp[3]").map(String::as_str), Some("42"));
        assert_eq!(p.get("duration[3]").map(String::as_str), Some("180"));
        assert_eq!(p.get("trackNumber[3]").map(String::as_str), Some("2"));
        assert_eq!(p.get("mbid[3]").map(String::as_str), Some("m"));
    }

    #[test]
    fn form_encoding_escapes_reserved_and_utf8() {
        let p = map(&[("a b", "x&y=z"), ("k", "é")]);
        assert_eq!(form_encode(&p), "a%20b=x%26y%3Dz&k=%C3%A9");
        let p = map(&[("artist[0]", "A-B_c.d~")]);
        assert_eq!(form_encode(&p), "artist%5B0%5D=A-B_c.d~");
    }

    #[test]
    fn error_classification() {
        assert!(matches!(classify_error(6, "x"), SubmitError::Permanent(_)));
        assert!(matches!(classify_error(9, "x"), SubmitError::Transient(_)));
        assert!(matches!(classify_error(29, "x"), SubmitError::Transient(_)));
    }

    #[test]
    fn required_settings() {
        let mut settings = toml::Table::new();
        let cfg = |settings: &toml::Table| IntegrationConfig {
            name: "fm".to_owned(),
            integration_type: "lastfm".to_owned(),
            enabled: true,
            settings: settings.clone(),
        };
        assert!(matches!(
            factory(&cfg(&settings)),
            Err(PluginError::Config(_))
        ));
        for k in ["api_key", "api_secret", "session_key"] {
            settings.insert(k.to_owned(), toml::Value::String("v".to_owned()));
        }
        assert!(factory(&cfg(&settings)).is_ok());
        settings.insert(
            "api_url".to_owned(),
            toml::Value::String("javascript:x".to_owned()),
        );
        assert!(factory(&cfg(&settings)).is_err());
    }
}
