// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Input-scheme SPI: [`InputPlugin`] plus the compile-time [`INPUT_PLUGINS`]
//! registry, keyed by URI scheme (`http`, `https`, ...). The decoder asks
//! [`open`] for any `scheme://` URI instead of hardcoding transports.

use std::io;

use symphonia::core::io::MediaSource;

use crate::TitleHandle;

/// Per-call state threaded through recursive opens (playlist unwrapping).
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenContext {
    /// Playlist nesting depth of this open (0 = the URI the user asked for).
    pub depth: u32,
    /// Ignore ICY titles: the URI, or a playlist it was unwrapped from,
    /// matched `[stream].metadata_blacklist`.
    pub metadata_blacklisted: bool,
}

/// An opened input, ready to hand to the decoder.
pub struct OpenedInput {
    /// The byte source.
    pub source: Box<dyn MediaSource>,
    /// "Now playing" title handle, when the transport carries one (ICY).
    pub title: Option<TitleHandle>,
    /// Format hint (file extension) for the demuxer probe, if known.
    pub extension_hint: Option<String>,
    /// The URI actually opened (after playlist unwrapping and redirects of
    /// the playlist chain; never logged, may carry credentials).
    pub uri: String,
}

/// A transport that can open URIs of one or more schemes.
pub trait InputPlugin: Send + Sync {
    /// Registry name, e.g. `"http"`.
    fn name(&self) -> &'static str;
    /// Lowercase URI schemes handled, without `://`, e.g. `["http", "https"]`.
    fn schemes(&self) -> &'static [&'static str];
    /// Open `uri`. Blocking; called from the decoder thread.
    ///
    /// # Errors
    /// Connection, protocol or unsupported-content failures.
    fn open(&self, uri: &str, ctx: &OpenContext) -> io::Result<OpenedInput>;
}

/// All compiled-in input plugins, in lookup priority order.
pub static INPUT_PLUGINS: &[&dyn InputPlugin] = &[&crate::http::HttpInput];

/// The lowercase scheme of `uri` when it has the form `scheme://...`.
#[must_use]
pub fn uri_scheme(uri: &str) -> Option<String> {
    let (scheme, _) = uri.split_once("://")?;
    let valid = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// Find the plugin registered for `scheme` (case-insensitive).
#[must_use]
pub fn input_for_scheme(scheme: &str) -> Option<&'static dyn InputPlugin> {
    let s = scheme.to_ascii_lowercase();
    INPUT_PLUGINS
        .iter()
        .copied()
        .find(|p| p.schemes().contains(&s.as_str()))
}

/// Find the plugin that handles `uri` by its scheme.
#[must_use]
pub fn input_for_uri(uri: &str) -> Option<&'static dyn InputPlugin> {
    input_for_scheme(&uri_scheme(uri)?)
}

/// Whether some compiled-in input plugin can open `uri`.
#[must_use]
pub fn is_input_uri(uri: &str) -> bool {
    input_for_uri(uri).is_some()
}

/// All handled URI prefixes (`"http://"`, `"https://"`, ...), registry order.
#[must_use]
pub fn url_handlers() -> Vec<String> {
    INPUT_PLUGINS
        .iter()
        .flat_map(|p| p.schemes().iter().map(|s| format!("{s}://")))
        .collect()
}

/// Open `uri` through the registry with a fresh context.
///
/// # Errors
/// `Unsupported` when no plugin handles the scheme; otherwise whatever the
/// plugin reports.
pub fn open(uri: &str) -> io::Result<OpenedInput> {
    open_with(uri, OpenContext::default())
}

/// Open `uri` through the registry with an explicit context (used for
/// recursive opens by playlist unwrapping).
///
/// # Errors
/// See [`open`].
pub fn open_with(uri: &str, ctx: OpenContext) -> io::Result<OpenedInput> {
    let Some(plugin) = input_for_uri(uri) else {
        let scheme = uri_scheme(uri).unwrap_or_else(|| "(none)".to_owned());
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported URI scheme `{scheme}`"),
        ));
    };
    plugin.open(uri, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_extraction() {
        assert_eq!(uri_scheme("http://h/x").as_deref(), Some("http"));
        assert_eq!(uri_scheme("HTTPS://h/x").as_deref(), Some("https"));
        assert_eq!(uri_scheme("/music/a.mp3"), None);
        assert_eq!(uri_scheme("://x"), None);
        assert_eq!(uri_scheme("/a/b://c"), None);
        assert_eq!(uri_scheme("C:\\music\\a.mp3"), None);
        assert_eq!(uri_scheme("1http://x"), None);
    }

    #[test]
    fn registry_lookup_by_scheme() {
        assert_eq!(input_for_uri("http://h/s").map(|p| p.name()), Some("http"));
        assert_eq!(input_for_uri("HTTPS://h/s").map(|p| p.name()), Some("http"));
        assert!(input_for_uri("mms://h/s").is_none());
        assert!(input_for_uri("/local/file.flac").is_none());
        assert!(is_input_uri("https://h/s"));
        assert!(!is_input_uri("file:///a.flac"));
    }

    #[test]
    fn registry_schemes_are_lowercase_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in INPUT_PLUGINS {
            for s in p.schemes() {
                assert_eq!(*s, s.to_ascii_lowercase());
                assert!(seen.insert(*s), "duplicate scheme {s}");
            }
        }
    }

    #[test]
    fn url_handlers_lists_http_and_https() {
        let h = url_handlers();
        assert!(h.contains(&"http://".to_owned()));
        assert!(h.contains(&"https://".to_owned()));
    }

    #[test]
    fn unknown_scheme_is_unsupported() {
        let err = open("gopher://h/x").err().expect("must fail");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
