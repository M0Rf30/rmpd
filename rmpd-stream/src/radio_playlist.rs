// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Radio playlist unwrapping: decide whether an HTTP(S) resource is a
//! playlist (`.pls`, `.m3u`, `.asx`, `.xspf`, ...) rather than audio, detect
//! unsupported HLS playlists, and walk the parsed entries until one opens.
//!
//! Everything here is pure (no I/O besides reading an already-open body), so
//! the decisions are unit-testable without a network.

use std::io::{self, Read};

use rmpd_plugin::playlist::{PlaylistEntry, PlaylistParser, parser_for_mime, parser_for_suffix};

/// Largest playlist body that will be fetched and parsed (1 MiB).
pub const MAX_PLAYLIST_BYTES: u64 = 1024 * 1024;

/// Maximum number of nested playlist levels (a playlist whose entry is itself
/// a playlist, ...). The fourth level is rejected.
pub const MAX_PLAYLIST_DEPTH: u32 = 3;

/// Tags that only occur in HLS (HTTP Live Streaming) playlists.
const HLS_TAGS: &[&str] = &[
    "#EXT-X-TARGETDURATION",
    "#EXT-X-STREAM-INF",
    "#EXT-X-MEDIA-SEQUENCE",
];

/// Extension of the URL's path (ignoring userinfo/host, query and fragment):
/// `http://h/x/song.mp3?b=1` → `Some("mp3")`; `http://example.com` → `None`.
#[must_use]
pub fn url_suffix(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let rest = rest.split(['?', '#']).next()?;
    let (_, path) = rest.split_once('/')?;
    let name = path.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    (!ext.is_empty()).then_some(ext)
}

/// MIME types that mean "this is audio, do not treat it as a playlist" even
/// when the URL suffix looks like a playlist.
fn is_audio_mime(mime: &str) -> bool {
    mime.starts_with("audio/") || mime.starts_with("video/") || mime == "application/ogg"
}

/// Decide whether the resource at `url` (served with `content_type`) is a
/// playlist, and which parser handles it.
///
/// 1. A `Content-Type` known to a parser wins.
/// 2. Any other audio/video/ogg `Content-Type` means "stream it", whatever
///    the suffix says.
/// 3. Otherwise (missing, `text/*`, `application/octet-stream`, ...) the URL
///    suffix decides.
#[must_use]
pub fn playlist_parser_for(
    url: &str,
    content_type: Option<&str>,
) -> Option<&'static dyn PlaylistParser> {
    let mime = content_type
        .map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|m| !m.is_empty());
    if let Some(m) = &mime {
        if let Some(parser) = parser_for_mime(m) {
            return Some(parser);
        }
        if is_audio_mime(m) {
            return None;
        }
    }
    url_suffix(url).and_then(parser_for_suffix)
}

/// Whether `text` is an HLS playlist (`#EXT-X-TARGETDURATION`,
/// `#EXT-X-STREAM-INF` or `#EXT-X-MEDIA-SEQUENCE`). HLS is not a flat list of
/// stream URLs, so parsing it as an M3U would yield bogus entries.
#[must_use]
pub fn is_hls_playlist(text: &str) -> bool {
    text.lines()
        .map(str::trim_start)
        .any(|l| HLS_TAGS.iter().any(|t| l.starts_with(t)))
}

/// Error returned for HLS playlists.
#[must_use]
pub fn hls_unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "HLS (HTTP Live Streaming) playlists are not supported",
    )
}

/// Read at most [`MAX_PLAYLIST_BYTES`] from `reader` as (lossy) UTF-8.
///
/// # Errors
/// Fails when the body is larger than the cap or the read fails.
pub fn read_capped(reader: impl Read, max: u64) -> io::Result<String> {
    let mut buf = Vec::new();
    reader.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("playlist larger than {max} bytes"),
        ));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Resolve a playlist entry against the playlist's own URL. Absolute URIs are
/// returned as written; relative references are joined onto `base`.
#[must_use]
pub fn resolve_entry(base: &str, entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if reqwest::Url::parse(entry).is_ok() {
        return Some(entry.to_owned());
    }
    reqwest::Url::parse(base)
        .ok()?
        .join(entry)
        .ok()
        .map(|u| u.to_string())
}

/// Try `entries` in order until `open` succeeds. Returns the last error when
/// every entry fails, or `NotFound` when there is nothing to try.
///
/// # Errors
/// See above.
pub fn try_entries<T>(
    base: &str,
    entries: &[PlaylistEntry],
    mut open: impl FnMut(&str) -> io::Result<T>,
) -> io::Result<T> {
    let mut last_err: Option<io::Error> = None;
    for entry in entries {
        let Some(target) = resolve_entry(base, &entry.uri) else {
            continue;
        };
        match open(&target) {
            Ok(v) => return Ok(v),
            Err(e) => {
                tracing::debug!(
                    entry = %crate::redact_url(&target),
                    error = %e,
                    "playlist entry failed to open"
                );
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "playlist contains no playable entries",
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn name(p: Option<&'static dyn PlaylistParser>) -> Option<&'static str> {
        p.map(|p| p.name())
    }

    fn entry(uri: &str) -> PlaylistEntry {
        PlaylistEntry {
            uri: uri.to_owned(),
            title: None,
            duration: None,
        }
    }

    #[test]
    fn suffix_decides_without_content_type() {
        assert_eq!(
            name(playlist_parser_for("http://h/a.pls", None)),
            Some("pls")
        );
        assert_eq!(
            name(playlist_parser_for("http://h/a.m3u", None)),
            Some("m3u")
        );
        assert_eq!(
            name(playlist_parser_for("http://h/a.m3u8?token=1", None)),
            Some("m3u")
        );
        assert_eq!(
            name(playlist_parser_for("http://h/a.asx", None)),
            Some("asx")
        );
        assert_eq!(
            name(playlist_parser_for("http://h/a.XSPF#frag", None)),
            Some("xspf")
        );
        assert_eq!(name(playlist_parser_for("http://h/stream", None)), None);
        assert_eq!(name(playlist_parser_for("http://h/a.mp3", None)), None);
        assert_eq!(name(playlist_parser_for("http://example.com", None)), None);
    }

    #[test]
    fn content_type_decides() {
        assert_eq!(
            name(playlist_parser_for(
                "http://h/listen",
                Some("audio/x-scpls")
            )),
            Some("pls")
        );
        assert_eq!(
            name(playlist_parser_for(
                "http://h/listen",
                Some("Audio/X-MPEGURL; charset=utf-8")
            )),
            Some("m3u")
        );
        assert_eq!(
            name(playlist_parser_for(
                "http://h/x",
                Some("application/vnd.apple.mpegurl")
            )),
            Some("m3u")
        );
        assert_eq!(
            name(playlist_parser_for(
                "http://h/x",
                Some("application/xspf+xml")
            )),
            Some("xspf")
        );
    }

    #[test]
    fn audio_content_type_beats_playlist_suffix() {
        assert_eq!(
            name(playlist_parser_for("http://h/a.m3u", Some("audio/mpeg"))),
            None
        );
        assert_eq!(
            name(playlist_parser_for(
                "http://h/a.pls",
                Some("application/ogg")
            )),
            None
        );
    }

    #[test]
    fn generic_content_type_falls_back_to_suffix() {
        for ct in ["text/plain", "application/octet-stream", "text/html", ""] {
            assert_eq!(
                name(playlist_parser_for("http://h/a.pls", Some(ct))),
                Some("pls"),
                "{ct}"
            );
            assert_eq!(name(playlist_parser_for("http://h/stream", Some(ct))), None);
        }
    }

    #[test]
    fn hls_detection() {
        assert!(is_hls_playlist(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nseg1.ts\n"
        ));
        assert!(is_hls_playlist(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nlow/index.m3u8\n"
        ));
        assert!(is_hls_playlist("#EXTM3U\r\n  #EXT-X-MEDIA-SEQUENCE:0\r\n"));
        // Plain and extended radio M3Us are not HLS.
        assert!(!is_hls_playlist(
            "#EXTM3U\n#EXTINF:-1,Radio\nhttp://h/stream\n"
        ));
        assert!(!is_hls_playlist("http://h/stream\n"));
        assert!(!is_hls_playlist(""));
    }

    #[test]
    fn hls_error_is_unsupported() {
        let e = hls_unsupported();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("HLS"));
    }

    #[test]
    fn url_suffix_cases() {
        assert_eq!(url_suffix("http://h/x/song.mp3?b=1"), Some("mp3"));
        assert_eq!(url_suffix("http://h/x/song.mp3#t"), Some("mp3"));
        assert_eq!(url_suffix("http://h/x.y/stream"), None);
        assert_eq!(url_suffix("http://example.com"), None);
        assert_eq!(url_suffix("http://example.com/"), None);
        assert_eq!(url_suffix("/local/file.flac"), None);
        assert_eq!(url_suffix("http://h/a."), None);
    }

    #[test]
    fn read_capped_enforces_limit() {
        assert_eq!(read_capped(Cursor::new(b"hello"), 5).unwrap(), "hello");
        let err = read_capped(Cursor::new(b"hello!"), 5).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // Invalid UTF-8 is replaced, not fatal.
        assert!(
            read_capped(Cursor::new(vec![0xff, b'a']), 10)
                .unwrap()
                .contains('a')
        );
    }

    #[test]
    fn resolve_entry_absolute_and_relative() {
        assert_eq!(
            resolve_entry("http://h/dir/list.m3u", "http://other/s").as_deref(),
            Some("http://other/s")
        );
        assert_eq!(
            resolve_entry("http://h/dir/list.m3u", "stream.mp3").as_deref(),
            Some("http://h/dir/stream.mp3")
        );
        assert_eq!(
            resolve_entry("http://h/dir/list.m3u", "/root.mp3").as_deref(),
            Some("http://h/root.mp3")
        );
        assert_eq!(resolve_entry("http://h/l.m3u", "   "), None);
    }

    #[test]
    fn try_entries_returns_first_success() {
        let entries = [
            entry("http://a/1"),
            entry("http://b/2"),
            entry("http://c/3"),
        ];
        let mut tried = Vec::new();
        let got = try_entries("http://h/l.pls", &entries, |u| {
            tried.push(u.to_owned());
            if u.contains("//b/") {
                Ok(u.to_owned())
            } else {
                Err(io::Error::other("down"))
            }
        })
        .unwrap();
        assert_eq!(got, "http://b/2");
        assert_eq!(tried, ["http://a/1", "http://b/2"]);
    }

    #[test]
    fn try_entries_reports_last_error_or_not_found() {
        let entries = [entry("http://a/1"), entry("http://b/2")];
        let err = try_entries::<()>("http://h/l.pls", &entries, |u| {
            Err(io::Error::other(format!("fail {u}")))
        })
        .unwrap_err();
        assert!(err.to_string().contains("http://b/2"));

        let err = try_entries::<()>("http://h/l.pls", &[], |_| Ok(())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        // Blank entries are skipped, not opened.
        let err = try_entries::<()>("http://h/l.pls", &[entry("  ")], |_| Ok(())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn depth_limit_is_three() {
        assert_eq!(MAX_PLAYLIST_DEPTH, 3);
    }
}
