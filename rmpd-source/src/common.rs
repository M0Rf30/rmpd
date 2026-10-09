// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helpers shared by the HTTP-based sources (`jellyfin`, `podcast`, `radio`,
//! `somafm`): virtual-path encoding, URL query encoding, `Song` construction,
//! HTTP client construction and error mapping.
//!
//! Compiled only when at least one of those features is enabled.

#![allow(dead_code)]

use camino::Utf8PathBuf;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::{Song, intern_tag_key};
use rmpd_plugin::source::{SourceError, SourceResult};
use std::time::Duration;

/// `User-Agent` sent by every HTTP source: `rmpd/<version>`.
pub const USER_AGENT: &str = concat!("rmpd/", env!("CARGO_PKG_VERSION"));

// ─── Virtual path segments ───────────────────────────────────────────────────

/// Percent-encode characters that would break the virtual path scheme
/// (`%` and `/`), so splitting on `/` always yields intact segments.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            _ => out.push(c),
        }
    }
    out
}

/// Inverse of [`enc`] (decodes every valid `%XX` sequence; invalid sequences
/// are kept verbatim).
pub fn dec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Split a source-relative browse directory into decoded, non-empty segments.
pub fn dir_segments(dir: &str) -> Vec<String> {
    dir.split('/').filter(|s| !s.is_empty()).map(dec).collect()
}

// ─── URL encoding ────────────────────────────────────────────────────────────

/// RFC 3986 percent-encoding of a query/path component (unreserved
/// characters are kept, everything else is `%XX` per UTF-8 byte).
pub fn urlenc(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
    }
    out
}

/// Join `key=value` pairs (values percent-encoded) with `&`.
pub fn query_string(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlenc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Strip trailing `/` and validate an `http(s)://` base URL.
pub fn normalize_base_url(raw: &str, what: &str) -> SourceResult<String> {
    let t = raw.trim().trim_end_matches('/');
    if !(t.starts_with("http://") || t.starts_with("https://")) {
        return Err(SourceError::Config(format!(
            "{what} `url` must start with http:// or https://"
        )));
    }
    Ok(t.to_owned())
}

// ─── Audio helpers ───────────────────────────────────────────────────────────

/// Pick a file extension known to `extract_remote_id` from a container /
/// codec string (may be a comma list such as `"mov,mp4,m4a,3gp"`).
pub fn known_ext(container: &str) -> Option<String> {
    container
        .split(|c: char| c == ',' || c == '|' || c.is_whitespace())
        .map(|p| p.trim().trim_start_matches('.').to_ascii_lowercase())
        .find(|p| crate::AUDIO_EXTENSIONS.contains(&p.as_str()))
}

/// Map a common audio MIME type to a known file extension.
pub fn mime_to_ext(mime: &str) -> Option<&'static str> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    Some(match base.as_str() {
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg-3" => "mp3",
        "audio/ogg" | "application/ogg" | "audio/vorbis" => "ogg",
        "audio/opus" => "opus",
        "audio/aac" | "audio/aacp" | "audio/x-aac" => "aac",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/x-ape" | "audio/ape" | "audio/x-monkeys-audio" => "ape",
        "audio/x-wavpack" | "audio/wavpack" => "wv",
        "audio/x-ms-wma" => "wma",
        "audio/aiff" | "audio/x-aiff" => "aiff",
        _ => return None,
    })
}

/// The id of a mount-style path leaf: last `/` segment minus a known audio
/// extension. Mirrors `SourceRegistry`'s extraction.
pub fn leaf_id(path: &str) -> &str {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if let Some((stem, ext)) = leaf.rsplit_once('.')
        && crate::AUDIO_EXTENSIONS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
    {
        return stem;
    }
    leaf
}

// ─── Song construction ───────────────────────────────────────────────────────

/// Build a `Song` with a virtual `path`; empty tag values are dropped.
pub fn make_song(
    path: String,
    tags: Vec<(&str, String)>,
    duration: Option<Duration>,
    bitrate: Option<u32>,
) -> Song {
    Song {
        id: 0,
        path: Utf8PathBuf::from(path),
        duration,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        range: None,
        tags: tags
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (intern_tag_key(k), v))
            .collect(),
    }
}

// ─── HTTP ────────────────────────────────────────────────────────────────────

/// Build the shared HTTP client (`User-Agent: rmpd/<version>`).
pub fn http_client(timeout: Duration, accept_invalid_certs: bool) -> SourceResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .danger_accept_invalid_certs(accept_invalid_certs)
        .build()
        .map_err(|e| SourceError::Config(format!("cannot build HTTP client: {e}")))
}

/// Map a transport error to `SourceError::Unreachable`, stripping the URL
/// (it may carry an `api_key` query parameter).
pub fn map_reqwest_err(e: reqwest::Error) -> SourceError {
    SourceError::Unreachable(e.without_url().to_string())
}

/// Map a non-success HTTP status to a `SourceError` (no URL, no body).
pub fn status_error(status: reqwest::StatusCode) -> SourceError {
    match status.as_u16() {
        401 | 403 => SourceError::Auth(format!("HTTP {status}")),
        404 => SourceError::NotFound(format!("HTTP {status}")),
        500..=599 => SourceError::Unreachable(format!("HTTP {status}")),
        _ => SourceError::Protocol(format!("HTTP {status}")),
    }
}

/// Send a request and return the response body, mapping failures to
/// `SourceError`.
pub async fn send_bytes(req: reqwest::RequestBuilder) -> SourceResult<Vec<u8>> {
    let resp = req.send().await.map_err(map_reqwest_err)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(status_error(status));
    }
    let bytes = resp.bytes().await.map_err(map_reqwest_err)?;
    Ok(bytes.to_vec())
}

// ─── Settings ────────────────────────────────────────────────────────────────

/// Read a list-valued setting: a TOML array of strings, or a single string.
pub fn setting_list(cfg: &SourceConfig, key: &str) -> Vec<String> {
    match cfg.settings.get(key) {
        Some(toml::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(toml::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_owned()]
            }
        }
        _ => Vec::new(),
    }
}

/// Read a boolean setting (`true`/`false`, case-insensitive).
pub fn setting_bool(cfg: &SourceConfig, key: &str, default: bool) -> bool {
    match cfg
        .setting_str(key)
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("true" | "yes" | "1") => true,
        Some("false" | "no" | "0") => false,
        _ => default,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enc_dec_roundtrip() {
        let s = "AC/DC 100% live";
        let e = enc(s);
        assert_eq!(e, "AC%2FDC 100%25 live");
        assert_eq!(dec(&e), s);
        assert_eq!(dec("100%"), "100%");
        assert_eq!(dec("a%zzb"), "a%zzb");
    }

    #[test]
    fn dir_segments_decodes() {
        assert_eq!(
            dir_segments("countries/AC%2FDC/"),
            vec!["countries".to_owned(), "AC/DC".to_owned()]
        );
        assert!(dir_segments("").is_empty());
    }

    #[test]
    fn urlenc_escapes_reserved() {
        assert_eq!(urlenc("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(urlenc("Az09-_.~"), "Az09-_.~");
        assert_eq!(query_string(&[("q", "a b"), ("n", "1")]), "q=a%20b&n=1");
    }

    #[test]
    fn base_url_validation() {
        assert_eq!(
            normalize_base_url(" https://x.example/jf/ ", "jellyfin").unwrap(),
            "https://x.example/jf"
        );
        assert!(normalize_base_url("x.example", "jellyfin").is_err());
    }

    #[test]
    fn known_ext_from_container_list() {
        assert_eq!(known_ext("mov,mp4,m4a,3gp,3g2,mj2").as_deref(), Some("mp4"));
        assert_eq!(known_ext("FLAC").as_deref(), Some("flac"));
        assert_eq!(known_ext("matroska,webm"), None);
    }

    #[test]
    fn mime_and_leaf_id() {
        assert_eq!(mime_to_ext("audio/mpeg; charset=x"), Some("mp3"));
        assert_eq!(mime_to_ext("video/mp4"), None);
        assert_eq!(leaf_id("home/Artist/Album/abc123.flac"), "abc123");
        assert_eq!(leaf_id("home/Artist/Album/abc123"), "abc123");
    }

    #[test]
    fn make_song_drops_empty_tags() {
        let s = make_song(
            "m/a/b.mp3".to_owned(),
            vec![("title", "T".to_owned()), ("artist", String::new())],
            Some(Duration::from_secs(3)),
            Some(128),
        );
        assert_eq!(s.tags.len(), 1);
        assert_eq!(s.path.as_str(), "m/a/b.mp3");
        assert_eq!(s.bitrate, Some(128));
    }

    #[test]
    fn setting_list_forms() {
        let mut t = toml::Table::new();
        t.insert(
            "feeds".to_owned(),
            toml::Value::Array(vec![
                toml::Value::String(" http://a ".to_owned()),
                toml::Value::String(String::new()),
                toml::Value::String("http://b".to_owned()),
            ]),
        );
        t.insert("one".to_owned(), toml::Value::String("x.opml".to_owned()));
        t.insert("flag".to_owned(), toml::Value::Boolean(true));
        let cfg = SourceConfig {
            name: "n".to_owned(),
            source_type: "podcast".to_owned(),
            enabled: true,
            settings: t,
        };
        assert_eq!(setting_list(&cfg, "feeds"), vec!["http://a", "http://b"]);
        assert_eq!(setting_list(&cfg, "one"), vec!["x.opml"]);
        assert!(setting_list(&cfg, "none").is_empty());
        assert!(setting_bool(&cfg, "flag", false));
        assert!(!setting_bool(&cfg, "none", false));
    }
}
