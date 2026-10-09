// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Playlist-format SPI: [`PlaylistParser`] plus the compile-time
//! [`PLAYLIST_PLUGINS`] registry (M3U/extended M3U, PLS, XSPF, ASX).
//!
//! Parsers are pure functions over text: no I/O, no URI resolution against
//! `base_uri` unless a format requires it (the built-in parsers return
//! entries exactly as written, apart from stripping `file://` prefixes).

use std::time::Duration;

/// One entry of a parsed playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// Location as written in the playlist (`file://` prefix already removed).
    pub uri: String,
    /// Display title, when the format carries one.
    pub title: Option<String>,
    /// Duration, when the format carries a known (positive) one.
    pub duration: Option<Duration>,
}

impl PlaylistEntry {
    fn bare(uri: String) -> Self {
        Self {
            uri,
            title: None,
            duration: None,
        }
    }
}

/// A playlist format implementation.
pub trait PlaylistParser: Send + Sync {
    /// Registry name, e.g. `"m3u"`.
    fn name(&self) -> &'static str;
    /// Lowercase file suffixes without the dot, e.g. `["m3u", "m3u8"]`.
    fn suffixes(&self) -> &'static [&'static str];
    /// Lowercase MIME types, e.g. `["audio/x-mpegurl"]`.
    fn mime_types(&self) -> &'static [&'static str];
    /// Parse `content`. `base_uri` is the URI/path the playlist was fetched
    /// from (informational; may be empty). Malformed input yields fewer
    /// entries, never an error.
    fn parse(&self, base_uri: &str, content: &str) -> Vec<PlaylistEntry>;
}

/// All compiled-in playlist parsers, in lookup priority order.
pub static PLAYLIST_PLUGINS: &[&dyn PlaylistParser] =
    &[&M3uParser, &PlsParser, &XspfParser, &AsxParser];

/// Find a parser by file suffix (case-insensitive, leading `.` optional).
#[must_use]
pub fn parser_for_suffix(suffix: &str) -> Option<&'static dyn PlaylistParser> {
    let s = suffix.trim_start_matches('.').to_ascii_lowercase();
    PLAYLIST_PLUGINS
        .iter()
        .copied()
        .find(|p| p.suffixes().contains(&s.as_str()))
}

/// Find a parser by MIME type (case-insensitive, `; params` ignored).
#[must_use]
pub fn parser_for_mime(mime: &str) -> Option<&'static dyn PlaylistParser> {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    PLAYLIST_PLUGINS
        .iter()
        .copied()
        .find(|p| p.mime_types().contains(&m.as_str()))
}

/// Find a parser by registry name.
#[must_use]
pub fn parser_by_name(name: &str) -> Option<&'static dyn PlaylistParser> {
    PLAYLIST_PLUGINS.iter().copied().find(|p| p.name() == name)
}

/// Strip `file://localhost`, `file:///` and `file://` prefixes.
fn strip_file_uri_prefix(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("file://localhost") {
        rest.to_string()
    } else if let Some(rest) = value.strip_prefix("file:///") {
        format!("/{rest}")
    } else if let Some(rest) = value.strip_prefix("file://") {
        rest.to_string()
    } else {
        value.to_string()
    }
}

// ─── M3U / extended M3U ──────────────────────────────────────────────────────

/// Plain and extended M3U. `#EXTINF` metadata is attached only when the first
/// line is exactly `#EXTM3U` (mirrors MPD's `ExtM3uPlaylistPlugin`).
pub struct M3uParser;

/// Parse the payload of an `#EXTINF:` line: `<duration>,<title>` split on the
/// first comma. Returns `(title, duration)`; `None` when malformed or when
/// the line carries no information. A duration <= 0 means "unknown".
fn extm3u_parse_tag(line: &str) -> Option<(Option<String>, Option<Duration>)> {
    let comma = line.find(',')?;
    let duration: i64 = line[..comma].trim().parse().ok()?;
    let duration = (duration > 0).then(|| Duration::from_secs(duration.unsigned_abs()));
    let title = line[comma + 1..].trim_start();
    let title = (!title.is_empty()).then(|| title.to_string());
    if title.is_none() && duration.is_none() {
        return None;
    }
    Some((title, duration))
}

impl PlaylistParser for M3uParser {
    fn name(&self) -> &'static str {
        "m3u"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &["m3u", "m3u8"]
    }

    fn mime_types(&self) -> &'static [&'static str] {
        &[
            "audio/x-mpegurl",
            "audio/mpegurl",
            "application/x-mpegurl",
            "application/vnd.apple.mpegurl",
        ]
    }

    fn parse(&self, _base_uri: &str, content: &str) -> Vec<PlaylistEntry> {
        let mut lines = content.lines().peekable();
        let extended = lines.peek().is_some_and(|l| l.trim_end() == "#EXTM3U");
        let mut out = Vec::new();
        let mut pending: Option<(Option<String>, Option<Duration>)> = None;
        for line in lines {
            if extended && let Some(rest) = line.strip_prefix("#EXTINF:") {
                pending = extm3u_parse_tag(rest);
                continue;
            }
            if line.trim_start().starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut entry = PlaylistEntry::bare(line.to_string());
            if let Some((title, duration)) = pending.take() {
                entry.title = title;
                entry.duration = duration;
            }
            out.push(entry);
        }
        out
    }
}

// ─── PLS ─────────────────────────────────────────────────────────────────────

/// Winamp/Shoutcast `.pls` (`FileN=` lines).
pub struct PlsParser;

impl PlaylistParser for PlsParser {
    fn name(&self) -> &'static str {
        "pls"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &["pls"]
    }

    fn mime_types(&self) -> &'static [&'static str] {
        &["audio/x-scpls", "application/pls+xml"]
    }

    fn parse(&self, _base_uri: &str, content: &str) -> Vec<PlaylistEntry> {
        let mut out = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some((key, value)) = trimmed.split_once('=')
                && key.trim().len() >= 4
                && key.trim()[..4].eq_ignore_ascii_case("file")
            {
                out.push(PlaylistEntry::bare(strip_file_uri_prefix(value.trim())));
            }
        }
        out
    }
}

// ─── XSPF ────────────────────────────────────────────────────────────────────

/// XSPF (`<location>` elements; falls back to `<file>`).
pub struct XspfParser;

/// Decode the five predefined XML entities and numeric character references
/// in a single pass; unknown or malformed references are kept verbatim.
fn decode_xml_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..].find(';').filter(|&n| n <= 10).and_then(|n| {
            let name = &rest[1..=n];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name.strip_prefix('#').and_then(|num| {
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => num.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            ch.map(|c| (c, n + 2))
        });
        if let Some((c, len)) = decoded {
            out.push(c);
            rest = &rest[len..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn extract_xml_tag_content(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut results = Vec::new();
    let mut remaining = xml;
    while let Some(start) = remaining.find(&open) {
        let after_open = &remaining[start + open.len()..];
        if let Some(end) = after_open.find(&close) {
            results.push(after_open[..end].trim().to_string());
            remaining = &after_open[end + close.len()..];
        } else {
            break;
        }
    }
    results
}

impl PlaylistParser for XspfParser {
    fn name(&self) -> &'static str {
        "xspf"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &["xspf"]
    }

    fn mime_types(&self) -> &'static [&'static str] {
        &["application/xspf+xml"]
    }

    fn parse(&self, _base_uri: &str, content: &str) -> Vec<PlaylistEntry> {
        let mut paths = extract_xml_tag_content(content, "location");
        if paths.is_empty() {
            paths = extract_xml_tag_content(content, "file");
        }
        paths
            .into_iter()
            .map(|p| {
                PlaylistEntry::bare(strip_file_uri_prefix(decode_xml_entities(p.trim()).trim()))
            })
            .collect()
    }
}

// ─── ASX ─────────────────────────────────────────────────────────────────────

/// Windows Media `.asx` (`<REF HREF="..."/>`, case-insensitive).
pub struct AsxParser;

impl PlaylistParser for AsxParser {
    fn name(&self) -> &'static str {
        "asx"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &["asx"]
    }

    fn mime_types(&self) -> &'static [&'static str] {
        &["video/x-ms-asf", "audio/x-ms-asx", "video/x-ms-asx"]
    }

    fn parse(&self, _base_uri: &str, content: &str) -> Vec<PlaylistEntry> {
        let mut out = Vec::new();
        let mut remaining = content;
        while let Some(pos) = remaining.to_ascii_lowercase().find("<ref ") {
            let chunk = &remaining[pos..];
            if let Some(href_pos) = chunk.to_ascii_lowercase().find("href=") {
                let after_href = &chunk[href_pos + 5..];
                let trimmed = after_href.trim_start_matches(|c: char| c.is_ascii_whitespace());
                let (quote, rest) = if let Some(s) = trimmed.strip_prefix('"') {
                    ('"', s)
                } else if let Some(s) = trimmed.strip_prefix('\'') {
                    ('\'', s)
                } else {
                    remaining = &remaining[pos + 5..];
                    continue;
                };
                if let Some(end) = rest.find(quote) {
                    out.push(PlaylistEntry::bare(strip_file_uri_prefix(
                        &decode_xml_entities(&rest[..end]),
                    )));
                }
            }
            remaining = &remaining[pos + 5..];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_by_suffix_and_mime() {
        assert_eq!(parser_for_suffix(".M3U8").unwrap().name(), "m3u");
        assert_eq!(parser_for_suffix("pls").unwrap().name(), "pls");
        assert!(parser_for_suffix("txt").is_none());
        assert_eq!(
            parser_for_mime("Audio/X-MpegURL; charset=utf-8")
                .unwrap()
                .name(),
            "m3u"
        );
        assert_eq!(
            parser_for_mime("application/xspf+xml").unwrap().name(),
            "xspf"
        );
        assert!(parser_for_mime("text/html").is_none());
    }

    #[test]
    fn m3u_plain_has_no_metadata() {
        let e = M3uParser.parse("", "# c\n\na.mp3\n#EXTINF:1,x\nb.mp3\n");
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|x| x.title.is_none() && x.duration.is_none()));
    }

    #[test]
    fn m3u_extended_metadata() {
        let e = M3uParser.parse(
            "",
            "#EXTM3U\n#EXTINF:123,Artist - Title\nhttp://x/a\n#EXTINF:-1,Live\nhttp://x/b\n#EXTINF:5,\nc\n#EXTINF:9,orphan\n",
        );
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].title.as_deref(), Some("Artist - Title"));
        assert_eq!(e[0].duration, Some(Duration::from_secs(123)));
        assert_eq!(e[1].duration, None);
        assert_eq!(e[1].title.as_deref(), Some("Live"));
        assert_eq!(e[2].title, None);
        assert_eq!(e[2].duration, Some(Duration::from_secs(5)));
    }

    #[test]
    fn pls_and_file_prefix() {
        let e = PlsParser.parse(
            "",
            "[playlist]\nFile1=file:///music/a.mp3\nTitle1=x\nfile2=http://h/s\n",
        );
        let uris: Vec<_> = e.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(uris, ["/music/a.mp3", "http://h/s"]);
    }

    #[test]
    fn xspf_location_then_file_fallback() {
        let e = XspfParser.parse("", "<track><location> http://h/a </location></track>");
        assert_eq!(e[0].uri, "http://h/a");
        let e = XspfParser.parse("", "<file>/m/a.mp3</file>");
        assert_eq!(e[0].uri, "/m/a.mp3");
    }

    #[test]
    fn asx_refs() {
        let e = AsxParser.parse(
            "",
            r#"<ASX><ENTRY><REF HREF="http://h/a"/></ENTRY><ref href='b.mp3'/></ASX>"#,
        );
        let uris: Vec<_> = e.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(uris, ["http://h/a", "b.mp3"]);
    }

    #[test]
    fn xml_entities_decoded() {
        assert_eq!(
            decode_xml_entities(
                "a&amp;b &lt;&gt;&quot;&apos; &#65;&#x42; &amp;amp; &bogus; &#xZZ; &"
            ),
            "a&b <>\"' AB &amp; &bogus; &#xZZ; &"
        );
        let e = XspfParser.parse("", "<location>http://h/s?sid=1&amp;type=mp3</location>");
        assert_eq!(e[0].uri, "http://h/s?sid=1&type=mp3");
        let e = AsxParser.parse("", r#"<REF HREF="http://h/s?sid=1&amp;type=mp3"/>"#);
        assert_eq!(e[0].uri, "http://h/s?sid=1&type=mp3");
    }
}
