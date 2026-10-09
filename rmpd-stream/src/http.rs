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
    MAX_PLAYLIST_BYTES, MAX_PLAYLIST_DEPTH, hls_unsupported, is_hls_playlist, playlist_parser_for,
    read_capped, try_entries, url_suffix,
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
        let body = read_capped(resp, MAX_PLAYLIST_BYTES)?;
        if is_hls_playlist(&body) {
            return Err(hls_unsupported());
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
    let extension_hint = url_suffix(uri)
        .filter(|ext| rmpd_plugin::playlist::parser_for_suffix(ext).is_none())
        .map(str::to_owned);
    Ok(OpenedInput {
        source: Box::new(source),
        title: Some(title),
        extension_hint,
        uri: uri.to_owned(),
    })
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
}
