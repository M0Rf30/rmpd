// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure HLS (RFC 8216) playlist parsing and variant selection.
//!
//! Nothing here performs I/O: a playlist body and the URL it was fetched from
//! go in, resolved absolute segment/variant URLs come out. The fetching,
//! decryption and demuxing live in `hls.rs` and `ts.rs`.

use std::io;
use std::sync::Arc;

use crate::radio_playlist::resolve_entry;

/// A `START@OFFSET`-style byte range (`#EXT-X-BYTERANGE`, `BYTERANGE="n@o"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// Number of bytes.
    pub len: u64,
    /// Absolute offset of the first byte.
    pub offset: u64,
}

impl ByteRange {
    /// The value of an HTTP `Range` request header selecting this range.
    #[must_use]
    pub fn header_value(&self) -> String {
        format!(
            "bytes={}-{}",
            self.offset,
            self.offset + self.len.saturating_sub(1)
        )
    }
}

/// How a segment is encrypted (`#EXT-X-KEY` `METHOD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMethod {
    /// `AES-128`: whole-segment AES-128-CBC with PKCS#7 padding.
    Aes128,
    /// Any other method (`SAMPLE-AES`, ...): not supported.
    Unsupported(String),
}

/// An active `#EXT-X-KEY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInfo {
    /// Encryption method.
    pub method: KeyMethod,
    /// Absolute URL of the 16-byte key (never logged: may carry tokens).
    pub uri: Option<String>,
    /// Explicit IV; otherwise the segment's media sequence number is used.
    pub iv: Option<[u8; 16]>,
}

/// An active `#EXT-X-MAP` (fMP4/CMAF initialization section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapInfo {
    /// Absolute URL of the initialization section.
    pub uri: String,
    /// Optional byte range inside `uri`.
    pub range: Option<ByteRange>,
}

/// One media segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Absolute URL.
    pub uri: String,
    /// `#EXTINF` duration in seconds.
    pub duration: f64,
    /// Media sequence number.
    pub sequence: u64,
    /// Optional byte range inside `uri`.
    pub range: Option<ByteRange>,
    /// Encryption in effect for this segment.
    pub key: Option<Arc<KeyInfo>>,
    /// Initialization section in effect for this segment.
    pub map: Option<Arc<MapInfo>>,
    /// An `#EXT-X-DISCONTINUITY` precedes this segment.
    pub discontinuity: bool,
}

/// A parsed media playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaPlaylist {
    /// `#EXT-X-TARGETDURATION` in seconds (0 when absent).
    pub target_duration: u64,
    /// Sequence number of the first segment.
    pub media_sequence: u64,
    /// `#EXT-X-ENDLIST` present: no more segments will ever appear.
    pub end_list: bool,
    /// Segments, oldest first.
    pub segments: Vec<Segment>,
}

/// One `#EXT-X-STREAM-INF` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// Absolute media playlist URL.
    pub uri: String,
    /// `BANDWIDTH` (falls back to `AVERAGE-BANDWIDTH`), bits per second.
    pub bandwidth: u64,
    /// `CODECS` attribute, if any.
    pub codecs: Option<String>,
    /// A `RESOLUTION` attribute is present.
    pub has_resolution: bool,
    /// `AUDIO` rendition group id.
    pub audio_group: Option<String>,
}

impl Variant {
    /// Whether the variant carries audio only: no `RESOLUTION`, and when
    /// `CODECS` is given every codec is an audio codec.
    #[must_use]
    pub fn is_audio_only(&self) -> bool {
        if self.has_resolution {
            return false;
        }
        match &self.codecs {
            Some(c) if !c.trim().is_empty() => {
                c.split(',').all(|codec| is_audio_codec(codec.trim()))
            }
            _ => true,
        }
    }
}

/// Whether an RFC 6381 codec identifier is an audio codec.
fn is_audio_codec(codec: &str) -> bool {
    let c = codec.to_ascii_lowercase();
    c.starts_with("mp4a.")
        || c.starts_with("ac-3")
        || c.starts_with("ec-3")
        || c.starts_with("ac-4")
        || c.starts_with("opus")
        || c.starts_with("flac")
        || c.starts_with("mp3")
        || c.starts_with("alac")
        || c.starts_with("vorbis")
}

/// One `#EXT-X-MEDIA:TYPE=AUDIO` rendition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRendition {
    /// `GROUP-ID`.
    pub group: String,
    /// Absolute playlist URL; renditions muxed into the variant have none.
    pub uri: Option<String>,
    /// `DEFAULT=YES`.
    pub default: bool,
}

/// A parsed master playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterPlaylist {
    /// Variant streams in document order.
    pub variants: Vec<Variant>,
    /// Alternative audio renditions.
    pub audio: Vec<AudioRendition>,
}

/// Either kind of playlist.
#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    /// `#EXT-X-STREAM-INF` playlist.
    Master(MasterPlaylist),
    /// Segment list.
    Media(MediaPlaylist),
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Split an HLS attribute list (`KEY=VALUE,KEY="quoted, value"`) into pairs.
/// Quotes are stripped; keys are upper-cased.
#[must_use]
pub fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().to_ascii_uppercase();
        let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((v, r)) => (v.to_owned(), r),
                None => (quoted.to_owned(), ""),
            }
        } else {
            match after.split_once(',') {
                Some((v, r)) => (v.trim().to_owned(), r),
                None => (after.trim().to_owned(), ""),
            }
        };
        out.push((key, value));
        rest = remainder.trim_start_matches(',').trim();
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Parse a `0x...` hexadecimal IV (up to 128 bits, left-padded with zeros).
#[must_use]
pub fn parse_iv(s: &str) -> Option<[u8; 16]> {
    let hex = s
        .trim()
        .strip_prefix("0x")
        .or_else(|| s.trim().strip_prefix("0X"))?;
    if hex.is_empty() || hex.len() > 32 {
        return None;
    }
    u128::from_str_radix(hex, 16).ok().map(u128::to_be_bytes)
}

/// The AES IV of a segment: the explicit one, or its media sequence number as
/// a big-endian 128-bit integer.
#[must_use]
pub fn segment_iv(key: &KeyInfo, sequence: u64) -> [u8; 16] {
    key.iv.unwrap_or_else(|| u128::from(sequence).to_be_bytes())
}

/// Parse `n[@o]` (an `#EXT-X-BYTERANGE` value) into `(length, offset)`.
fn parse_byte_spec(s: &str) -> Option<(u64, Option<u64>)> {
    let (len, offset) = match s.trim().split_once('@') {
        Some((l, o)) => (
            l.trim().parse::<u64>().ok()?,
            Some(o.trim().parse::<u64>().ok()?),
        ),
        None => (s.trim().parse::<u64>().ok()?, None),
    };
    (len > 0).then_some((len, offset))
}

/// Whether `text` is a master playlist.
fn is_master(text: &str) -> bool {
    text.lines()
        .any(|l| l.trim_start().starts_with("#EXT-X-STREAM-INF"))
}

/// Parse `text`, fetched from `base`, as a master or media playlist.
///
/// # Errors
/// `InvalidData` when `text` is not an `#EXTM3U` document.
pub fn parse(text: &str, base: &str) -> io::Result<Playlist> {
    let text = text.trim_start_matches('\u{feff}');
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(invalid("not an HLS playlist (missing #EXTM3U)"));
    }
    if is_master(text) {
        Ok(Playlist::Master(parse_master(text, base)))
    } else {
        Ok(Playlist::Media(parse_media(text, base)))
    }
}

/// Parse a master playlist.
#[must_use]
pub fn parse_master(text: &str, base: &str) -> MasterPlaylist {
    let mut variants = Vec::new();
    let mut audio = Vec::new();
    let mut pending: Option<Vec<(String, String)>> = None;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(parse_attrs(rest));
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = parse_attrs(rest);
            if attr(&a, "TYPE").is_some_and(|t| t.eq_ignore_ascii_case("AUDIO"))
                && let Some(group) = attr(&a, "GROUP-ID")
            {
                audio.push(AudioRendition {
                    group: group.to_owned(),
                    uri: attr(&a, "URI").and_then(|u| resolve_entry(base, u)),
                    default: attr(&a, "DEFAULT").is_some_and(|d| d.eq_ignore_ascii_case("YES")),
                });
            }
        } else if line.is_empty() || line.starts_with('#') {
            // Other tags and comments do not terminate a pending STREAM-INF.
        } else if let Some(a) = pending.take()
            && let Some(uri) = resolve_entry(base, line)
        {
            let bandwidth = attr(&a, "BANDWIDTH")
                .or_else(|| attr(&a, "AVERAGE-BANDWIDTH"))
                .and_then(|b| b.parse().ok())
                .unwrap_or(0);
            variants.push(Variant {
                uri,
                bandwidth,
                codecs: attr(&a, "CODECS").map(str::to_owned),
                has_resolution: attr(&a, "RESOLUTION").is_some(),
                audio_group: attr(&a, "AUDIO").map(str::to_owned),
            });
        }
    }
    MasterPlaylist { variants, audio }
}

/// Choose the playlist URL to play from a master playlist.
///
/// * Audio-only variants are preferred; with none, every variant is a
///   candidate (video is simply discarded by the demuxer).
/// * With `max_bandwidth`: the highest candidate not above it, or the lowest
///   candidate when all exceed it.
/// * Without: the highest audio-only candidate, or the lowest overall when
///   the candidates carry video.
/// * A video variant that references an `AUDIO` rendition group with its own
///   playlist is replaced by that rendition (`DEFAULT=YES` first), so the
///   video is never downloaded.
#[must_use]
pub fn select_variant(master: &MasterPlaylist, max_bandwidth: Option<u64>) -> Option<String> {
    let audio_only: Vec<&Variant> = master
        .variants
        .iter()
        .filter(|v| v.is_audio_only())
        .collect();
    let all_audio = !audio_only.is_empty();
    let candidates: Vec<&Variant> = if all_audio {
        audio_only
    } else {
        master.variants.iter().collect()
    };
    let lowest = || candidates.iter().min_by_key(|v| v.bandwidth).copied();
    let highest = || candidates.iter().max_by_key(|v| v.bandwidth).copied();
    let chosen = match max_bandwidth {
        Some(cap) => candidates
            .iter()
            .filter(|v| v.bandwidth <= cap)
            .max_by_key(|v| v.bandwidth)
            .copied()
            .or_else(lowest),
        None if all_audio => highest(),
        None => lowest(),
    }?;

    if !chosen.is_audio_only()
        && let Some(group) = &chosen.audio_group
    {
        let mut renditions: Vec<&AudioRendition> = master
            .audio
            .iter()
            .filter(|r| &r.group == group && r.uri.is_some())
            .collect();
        renditions.sort_by_key(|r| !r.default);
        if let Some(uri) = renditions.first().and_then(|r| r.uri.clone()) {
            return Some(uri);
        }
    }
    Some(chosen.uri.clone())
}

/// Parse a media playlist.
#[must_use]
pub fn parse_media(text: &str, base: &str) -> MediaPlaylist {
    let mut pl = MediaPlaylist {
        target_duration: 0,
        media_sequence: 0,
        end_list: false,
        segments: Vec::new(),
    };
    let mut duration: Option<f64> = None;
    let mut pending_range: Option<(u64, Option<u64>)> = None;
    let mut discontinuity = false;
    let mut key: Option<Arc<KeyInfo>> = None;
    let mut map: Option<Arc<MapInfo>> = None;
    // End of the previous byte range, per URI, for `n` without `@o`.
    let mut prev_range_end: Option<(String, u64)> = None;

    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(tag) = line.strip_prefix('#') {
            if let Some(v) = tag.strip_prefix("EXT-X-TARGETDURATION:") {
                pl.target_duration = v.trim().parse::<f64>().map_or(0, |d| d.ceil() as u64);
            } else if let Some(v) = tag.strip_prefix("EXT-X-MEDIA-SEQUENCE:") {
                pl.media_sequence = v.trim().parse().unwrap_or(0);
            } else if tag.starts_with("EXT-X-ENDLIST") {
                pl.end_list = true;
            } else if let Some(v) = tag.strip_prefix("EXTINF:") {
                duration = v
                    .split(',')
                    .next()
                    .and_then(|d| d.trim().parse::<f64>().ok());
            } else if tag.starts_with("EXT-X-DISCONTINUITY") && !tag.contains("SEQUENCE") {
                discontinuity = true;
            } else if let Some(v) = tag.strip_prefix("EXT-X-BYTERANGE:") {
                pending_range = parse_byte_spec(v);
            } else if let Some(v) = tag.strip_prefix("EXT-X-KEY:") {
                key = parse_key(v, base);
            } else if let Some(v) = tag.strip_prefix("EXT-X-MAP:") {
                let a = parse_attrs(v);
                map = attr(&a, "URI")
                    .and_then(|u| resolve_entry(base, u))
                    .map(|uri| {
                        Arc::new(MapInfo {
                            uri,
                            range: attr(&a, "BYTERANGE")
                                .and_then(parse_byte_spec)
                                .and_then(|(len, off)| Some(ByteRange { len, offset: off? })),
                        })
                    });
            }
            continue;
        }

        // A URI line: the segment announced by the preceding `#EXTINF`.
        let Some(uri) = resolve_entry(base, line) else {
            continue;
        };
        let range = pending_range.take();
        let Some(d) = duration.take() else {
            continue;
        };
        let range = range.and_then(|(len, offset)| {
            let offset = offset.or_else(|| {
                prev_range_end
                    .as_ref()
                    .filter(|(u, _)| *u == uri)
                    .map(|(_, end)| *end)
            })?;
            Some(ByteRange { len, offset })
        });
        prev_range_end = range.map(|r| (uri.clone(), r.offset + r.len));
        let sequence = pl.media_sequence + pl.segments.len() as u64;
        pl.segments.push(Segment {
            uri,
            duration: d,
            sequence,
            range,
            key: key.clone(),
            map: map.clone(),
            discontinuity: std::mem::take(&mut discontinuity),
        });
    }
    pl
}

fn parse_key(attrs: &str, base: &str) -> Option<Arc<KeyInfo>> {
    let a = parse_attrs(attrs);
    let method = attr(&a, "METHOD")?;
    if method.eq_ignore_ascii_case("NONE") {
        return None;
    }
    let method = if method.eq_ignore_ascii_case("AES-128") {
        KeyMethod::Aes128
    } else {
        KeyMethod::Unsupported(method.to_owned())
    };
    Some(Arc::new(KeyInfo {
        method,
        uri: attr(&a, "URI").and_then(|u| resolve_entry(base, u)),
        iv: attr(&a, "IV").and_then(parse_iv),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://cdn.example/live/master.m3u8";

    const MASTER: &str = "#EXTM3U\n\
#EXT-X-VERSION:6\n\
#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.5\"\n\
audio_64k/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\n\
audio_128k/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=320000,CODECS=\"mp4a.40.2\"\n\
https://other.example/audio_320k.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n\
video_360/index.m3u8\n";

    fn master(text: &str) -> MasterPlaylist {
        match parse(text, BASE).expect("parses") {
            Playlist::Master(m) => m,
            Playlist::Media(_) => panic!("expected master"),
        }
    }

    fn media(text: &str, base: &str) -> MediaPlaylist {
        match parse(text, base).expect("parses") {
            Playlist::Media(m) => m,
            Playlist::Master(_) => panic!("expected media"),
        }
    }

    #[test]
    fn attribute_lists_handle_quoted_commas() {
        let a = parse_attrs("BANDWIDTH=1280000,CODECS=\"avc1.4d,mp4a.40.2\",NAME=x");
        assert_eq!(attr(&a, "BANDWIDTH"), Some("1280000"));
        assert_eq!(attr(&a, "CODECS"), Some("avc1.4d,mp4a.40.2"));
        assert_eq!(attr(&a, "NAME"), Some("x"));
    }

    #[test]
    fn master_variants_are_parsed_and_resolved() {
        let m = master(MASTER);
        assert_eq!(m.variants.len(), 4);
        assert_eq!(
            m.variants[0].uri,
            "https://cdn.example/live/audio_64k/index.m3u8"
        );
        assert_eq!(m.variants[2].uri, "https://other.example/audio_320k.m3u8");
        assert_eq!(m.variants[1].bandwidth, 128_000);
        assert!(m.variants[0].is_audio_only());
        assert!(!m.variants[3].is_audio_only());
    }

    #[test]
    fn selection_prefers_best_audio_only_without_cap() {
        let m = master(MASTER);
        assert_eq!(
            select_variant(&m, None).as_deref(),
            Some("https://other.example/audio_320k.m3u8")
        );
    }

    #[test]
    fn selection_honours_bandwidth_cap() {
        let m = master(MASTER);
        assert_eq!(
            select_variant(&m, Some(200_000)).as_deref(),
            Some("https://cdn.example/live/audio_128k/index.m3u8")
        );
        // Everything exceeds the cap: fall back to the lowest audio variant.
        assert_eq!(
            select_variant(&m, Some(10)).as_deref(),
            Some("https://cdn.example/live/audio_64k/index.m3u8")
        );
    }

    #[test]
    fn selection_without_audio_only_picks_lowest() {
        let m = master(
            "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720\nhi.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360\nlo.m3u8\n",
        );
        assert_eq!(
            select_variant(&m, None).as_deref(),
            Some("https://cdn.example/live/lo.m3u8")
        );
        assert_eq!(
            select_variant(&m, Some(5_000_000)).as_deref(),
            Some("https://cdn.example/live/hi.m3u8")
        );
    }

    #[test]
    fn video_variant_is_replaced_by_audio_rendition() {
        let m = master(
            "#EXTM3U\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"alt\",URI=\"alt.m3u8\"\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"main\",DEFAULT=YES,URI=\"main.m3u8\"\n\
#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360,AUDIO=\"aud\"\nv.m3u8\n",
        );
        assert_eq!(
            select_variant(&m, None).as_deref(),
            Some("https://cdn.example/live/main.m3u8")
        );
    }

    #[test]
    fn empty_master_selects_nothing() {
        let m = master("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\n");
        assert!(select_variant(&m, None).is_none());
    }

    #[test]
    fn live_media_playlist() {
        let p = media(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:2680\n\
#EXTINF:6.006,Title\nseg2680.ts\n#EXTINF:5.5,\nseg2681.ts\n",
            "https://h/a/live.m3u8",
        );
        assert_eq!(p.target_duration, 6);
        assert_eq!(p.media_sequence, 2680);
        assert!(!p.end_list);
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[0].uri, "https://h/a/seg2680.ts");
        assert_eq!(p.segments[0].sequence, 2680);
        assert!((p.segments[0].duration - 6.006).abs() < 1e-9);
        assert_eq!(p.segments[1].sequence, 2681);
    }

    #[test]
    fn vod_playlist_with_endlist() {
        let p = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXTINF:10,\na.aac\n#EXTINF:4,\nb.aac\n#EXT-X-ENDLIST\n",
            BASE,
        );
        assert!(p.end_list);
        assert_eq!(p.media_sequence, 0);
        assert_eq!(p.segments.len(), 2);
    }

    #[test]
    fn discontinuity_marks_next_segment_only() {
        let p = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\na.ts\n#EXT-X-DISCONTINUITY\n\
#EXTINF:4,\nb.ts\n#EXTINF:4,\nc.ts\n",
            BASE,
        );
        assert!(!p.segments[0].discontinuity);
        assert!(p.segments[1].discontinuity);
        assert!(!p.segments[2].discontinuity);
    }

    #[test]
    fn key_and_map_apply_to_following_segments() {
        let p = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n\
#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n\
#EXTINF:4,\na.m4s\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\nb.m4s\n",
            "https://h/p/x.m3u8",
        );
        let k = p.segments[0].key.as_ref().expect("key");
        assert_eq!(k.method, KeyMethod::Aes128);
        assert_eq!(k.uri.as_deref(), Some("https://h/p/k.bin"));
        assert_eq!(
            k.iv,
            Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
        );
        assert!(p.segments[1].key.is_none());
        let m = p.segments[1].map.as_ref().expect("map");
        assert_eq!(m.uri, "https://h/p/init.mp4");
        assert_eq!(
            m.range,
            Some(ByteRange {
                len: 720,
                offset: 0
            })
        );
    }

    #[test]
    fn unsupported_key_method_is_recorded() {
        let p = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"\n#EXTINF:4,\na.ts\n",
            BASE,
        );
        let k = p.segments[0].key.as_ref().expect("key");
        assert_eq!(k.method, KeyMethod::Unsupported("SAMPLE-AES".to_owned()));
    }

    #[test]
    fn iv_defaults_to_sequence_number() {
        let k = KeyInfo {
            method: KeyMethod::Aes128,
            uri: None,
            iv: None,
        };
        let mut want = [0u8; 16];
        want[15] = 7;
        assert_eq!(segment_iv(&k, 7), want);
        let explicit = KeyInfo {
            iv: Some([9; 16]),
            ..k
        };
        assert_eq!(segment_iv(&explicit, 7), [9; 16]);
    }

    #[test]
    fn short_iv_is_left_padded() {
        let mut want = [0u8; 16];
        want[15] = 0x1f;
        assert_eq!(parse_iv("0x1f"), Some(want));
        assert_eq!(parse_iv("1f"), None);
        assert_eq!(parse_iv("0x"), None);
        assert_eq!(parse_iv(&format!("0x{}", "f".repeat(33))), None);
    }

    #[test]
    fn byte_ranges_chain_within_one_uri() {
        let p = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\n#EXT-X-BYTERANGE:1000@0\nall.aac\n\
#EXTINF:4,\n#EXT-X-BYTERANGE:500\nall.aac\n",
            "https://h/x.m3u8",
        );
        assert_eq!(
            p.segments[0].range,
            Some(ByteRange {
                len: 1000,
                offset: 0
            })
        );
        assert_eq!(
            p.segments[1].range,
            Some(ByteRange {
                len: 500,
                offset: 1000
            })
        );
        assert_eq!(
            p.segments[1].range.expect("range").header_value(),
            "bytes=1000-1499"
        );
    }

    #[test]
    fn non_hls_text_is_rejected() {
        assert!(parse("<html></html>", BASE).is_err());
    }

    #[test]
    fn bom_and_crlf_are_tolerated() {
        let p = media(
            "\u{feff}#EXTM3U\r\n#EXT-X-TARGETDURATION:4\r\n#EXTINF:4,\r\na.ts\r\n#EXT-X-ENDLIST\r\n",
            BASE,
        );
        assert_eq!(p.segments.len(), 1);
        assert!(p.end_list);
    }
}
