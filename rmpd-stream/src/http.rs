// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `http`/`https` input plugin: connects (honouring `[stream]` timeout and
//! proxy), unwraps radio playlists, and streams audio with ICY de-interleaving.

use std::io;
use std::time::Duration;

use rmpd_core::config::StreamConfig;

use crate::HttpSource;
use crate::input::{InputPlugin, OpenContext, OpenedInput, open_with};
use crate::radio_playlist::{
    MAX_PLAYLIST_BYTES, MAX_PLAYLIST_DEPTH, is_hls_playlist, playlist_parser_for, read_capped,
    try_entries, url_suffix,
};
use crate::settings;
use crate::{redact_url, to_io};

/// Input plugin for `http://` and `https://` URIs.
pub struct HttpInput;

impl InputPlugin for HttpInput {
    fn name(&self) -> &'static str {
        "http"
    }

    fn schemes(&self) -> &'static [&'static str] {
        &["http", "https"]
    }

    fn open(&self, uri: &str, ctx: &OpenContext) -> io::Result<OpenedInput> {
        open_http(uri, ctx, &settings::current())
    }
}

/// Build the blocking client from `[stream]` settings.
///
/// reqwest's blocking client has no dedicated "total request" timeout:
/// `.timeout()` bounds the initial connect+headers wait *and*, per call, each
/// subsequent `Read::read()` on the body (it resets on every read). So the
/// configured value acts as a per-read/idle deadline, not a cap on total
/// stream duration: a server that keeps sending stays connected indefinitely;
/// one that goes silent mid-stream is dropped within this window instead of
/// wedging the decoder thread.
fn build_client(cfg: &StreamConfig) -> io::Result<reqwest::blocking::Client> {
    let timeout = Duration::from_millis(cfg.timeout_ms.max(1));
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .user_agent("rmpd");
    if let Some(p) = cfg.proxy.as_ref().filter(|p| !p.url.trim().is_empty()) {
        let mut proxy = reqwest::Proxy::all(p.url.trim()).map_err(to_io)?;
        if let Some(user) = p.username.as_deref().filter(|u| !u.is_empty()) {
            proxy = proxy.basic_auth(user, p.password.as_deref().unwrap_or(""));
        }
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(to_io)
}

fn open_http(uri: &str, ctx: &OpenContext, cfg: &StreamConfig) -> io::Result<OpenedInput> {
    let blacklisted =
        ctx.metadata_blacklisted || settings::is_metadata_blacklisted(&cfg.metadata_blacklist, uri);
    let client = build_client(cfg)?;
    let resp = client
        .get(uri)
        .header("Icy-MetaData", "1")
        .send()
        .map_err(to_io)?
        .error_for_status()
        .map_err(to_io)?;

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // Radio playlist: fetch, parse, open the first entry that works.
    if let Some(parser) = playlist_parser_for(uri, content_type.as_deref()) {
        if ctx.depth >= MAX_PLAYLIST_DEPTH {
            return Err(io::Error::other(format!(
                "playlists nested deeper than {MAX_PLAYLIST_DEPTH} levels"
            )));
        }
        let final_url = resp.url().to_string();
        let body = read_capped(resp, MAX_PLAYLIST_BYTES)?;
        if is_hls_playlist(&body) {
            tracing::debug!(url = %redact_url(uri), "opening HLS playlist");
            return crate::hls::open_hls(client, &final_url, &body, cfg, uri);
        }
        let entries = parser.parse(uri, &body);
        tracing::debug!(
            url = %redact_url(uri),
            parser = parser.name(),
            entries = entries.len(),
            "unwrapping radio playlist"
        );
        let child = OpenContext {
            depth: ctx.depth + 1,
            metadata_blacklisted: blacklisted,
        };
        return try_entries(uri, &entries, |target| open_with(target, child));
    }

    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0);
    tracing::debug!(
        url = %redact_url(uri),
        ?metaint,
        blacklisted,
        "opened HTTP stream"
    );
    let source = HttpSource::with_reader(Box::new(resp), metaint).ignore_title(blacklisted);
    let title = source.title_handle();
    // A playlist-looking suffix on audio content would only mislead the probe.
    let suffix_hint =
        url_suffix(uri).filter(|ext| rmpd_plugin::playlist::parser_for_suffix(ext).is_none());
    let extension_hint = extension_hint_for(suffix_hint, content_type.as_deref());
    Ok(OpenedInput {
        source: Box::new(source),
        title: Some(title),
        extension_hint,
        uri: uri.to_owned(),
    })
}

/// Probe hint (file extension) implied by an HTTP `Content-Type`.
///
/// LATM/LOAS AAC is not distinguishable from ADTS by URL suffix (both are
/// commonly served as `*.aac`), but the radio MIME types `audio/mp4a-latm` /
/// `audio/aac-latm` are specific to it, so they select the LOAS reader.
/// Everything else is only a fallback for URLs without a usable suffix.
fn content_type_hint(content_type: &str) -> Option<(&'static str, bool)> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // (extension, whether it is specific enough to override the URL suffix)
    match mime.as_str() {
        "audio/mp4a-latm" | "audio/x-mp4a-latm" | "audio/aac-latm" => Some(("latm", true)),
        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some(("aac", false)),
        "audio/mpeg" | "audio/mp3" | "audio/x-mpeg" => Some(("mp3", false)),
        "audio/ogg" | "application/ogg" => Some(("ogg", false)),
        // MPEG-TS / FLV / MPEG-PS carried over plain HTTP (HLS segments use their own demuxer).
        "video/mp2t" | "audio/mp2t" => Some(("ts", false)),
        "video/x-flv" | "audio/x-flv" => Some(("flv", false)),
        "video/mpeg" | "video/mp2p" | "video/x-mpeg" => Some(("mpg", false)),
        _ => None,
    }
}

/// Combine the URL-suffix hint with the `Content-Type` hint.
fn extension_hint_for(suffix_hint: Option<&str>, content_type: Option<&str>) -> Option<String> {
    match content_type.and_then(content_type_hint) {
        Some((ext, true)) => Some(ext.to_owned()),
        Some((ext, false)) => Some(suffix_hint.unwrap_or(ext).to_owned()),
        None => suffix_hint.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::config::ProxyConfig;

    #[test]
    fn client_builds_with_defaults() {
        assert!(build_client(&StreamConfig::default()).is_ok());
    }

    #[test]
    fn client_builds_with_authenticated_proxy() {
        let cfg = StreamConfig {
            proxy: Some(ProxyConfig {
                url: "http://proxy.example:3128".to_owned(),
                username: Some("u".to_owned()),
                password: Some("p".to_owned()),
            }),
            ..StreamConfig::default()
        };
        assert!(build_client(&cfg).is_ok());
    }

    #[test]
    fn blank_proxy_url_is_ignored() {
        let cfg = StreamConfig {
            proxy: Some(ProxyConfig {
                url: "  ".to_owned(),
                username: None,
                password: None,
            }),
            ..StreamConfig::default()
        };
        assert!(build_client(&cfg).is_ok());
    }

    #[test]
    fn latm_content_type_overrides_suffix() {
        for ct in [
            "audio/mp4a-latm",
            "Audio/MP4A-LATM; charset=binary",
            "audio/aac-latm",
        ] {
            assert_eq!(
                extension_hint_for(Some("aac"), Some(ct)).as_deref(),
                Some("latm"),
                "{ct}"
            );
            assert_eq!(extension_hint_for(None, Some(ct)).as_deref(), Some("latm"));
        }
    }

    #[test]
    fn generic_content_type_only_fills_missing_suffix() {
        assert_eq!(
            extension_hint_for(None, Some("audio/aacp")).as_deref(),
            Some("aac")
        );
        assert_eq!(
            extension_hint_for(Some("mp3"), Some("audio/aac")).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            extension_hint_for(None, Some("audio/mpeg; charset=x")).as_deref(),
            Some("mp3")
        );
        assert_eq!(extension_hint_for(None, Some("text/html")), None);
        assert_eq!(
            extension_hint_for(Some("flac"), None).as_deref(),
            Some("flac")
        );
        assert_eq!(extension_hint_for(None, None), None);
    }
}
