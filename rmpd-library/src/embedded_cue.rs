//! Embedded FLAC cue-sheet extraction.
//!
//! A whole-album FLAC rip often carries a `CUESHEET` describing how it splits
//! into tracks, either as free-form cue-sheet text in a `CUESHEET` Vorbis
//! comment, or as FLAC's own binary `CUESHEET` metadata block. This mirrors
//! MPD's `embcue` and `flac` playlist plugins
//! (`src/playlist/plugins/EmbeddedCuePlaylistPlugin.cxx`,
//! `src/playlist/plugins/FlacPlaylistPlugin.cxx`), which read exactly these
//! two sources in the same priority order, but exposes the result through
//! the library scanner (see `crate::scanner`) instead of the `load` command,
//! so the resulting tracks are browsable (`lsinfo`) and searchable like any
//! other song, per this fork's design (stock MPD never scans an embedded cue
//! sheet into the database — only `container_scan` decoder plugins, e.g. GME
//! and Sidplay, do that, and FLAC isn't one of them).
//!
//! Two sources are tried, in that priority order:
//!
//! 1. The `CUESHEET` Vorbis comment (raw cue-sheet text) — parsed with
//!    [`crate::cue::parse_cue`], which recovers per-track TITLE/PERFORMER.
//!    An embedded sheet's own `FILE` line is ignored and overwritten with the
//!    FLAC's own name: an embedded cue sheet always describes the file it is
//!    embedded in (`EmbeddedCuePlaylist::filename` in
//!    `EmbeddedCuePlaylistPlugin.cxx` does the same override).
//! 2. The binary `CUESHEET` metadata block, decoded by this fork's Symphonia
//!    (`symphonia-metadata::embedded::flac::read_flac_cuesheet_block`) into a
//!    `ChapterGroup` via `FormatReader::chapters()`. This form carries only
//!    start/end times (and, for CD-DA sheets, an ISRC) — no track title or
//!    performer — matching `FlacPlaylistPlugin.cxx`'s `ToSongEnumerator`,
//!    which likewise only sets a start/end time on each virtual song.

use crate::cue::{CueTrack, parse_cue};
use crate::metadata::MetadataExtractor;
use camino::Utf8PathBuf;
use std::time::Duration;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{ChapterGroupItem, MetadataOptions};

/// A CD-DA cuesheet's lead-out track (conventionally numbered 170) has no
/// analogue in `parse_cue`'s track list and no `INDEX` points of its own.
/// Symphonia doesn't surface the cuesheet's `is_audio` flag that would
/// otherwise identify it (see `symphonia-metadata/src/embedded/flac.rs`'s
/// "these values ... have no analogue in Symphonia" comment), so it's
/// dropped heuristically: the last chapter is treated as a lead-out when it
/// has no index children of its own and starts within this many seconds of
/// the stream's reported end.
const LEAD_OUT_TOLERANCE_SECS: f64 = 2.0;

/// Read `path`'s embedded cue sheet, if any, as a flat list of tracks.
///
/// Prefers the `CUESHEET` text comment (title/performer per track); falls
/// back to the binary `CUESHEET` metadata block (start/end times and a
/// positional track number only). Returns `None` when neither is present.
/// `container_duration` (the containing song's own duration, as already
/// extracted by `MetadataExtractor::extract_from_file`) fills in the last
/// track's end time when the cue sheet doesn't specify one.
pub fn read_embedded_cue_tracks(
    path: &Utf8PathBuf,
    container_duration: Option<Duration>,
) -> Option<Vec<CueTrack>> {
    let total_secs = container_duration.map(|d| d.as_secs_f64());

    if let Some(text) = read_cuesheet_comment(path) {
        let mut tracks = parse_cue(&text);
        if !tracks.is_empty() {
            // An embedded sheet's FILE line is usually the original disc
            // image's filename (sometimes absent entirely): every track
            // always refers to the file it's embedded in, never that name.
            let file_name = path.file_name().unwrap_or_default().to_string();
            for t in &mut tracks {
                t.file = file_name.clone();
            }
            if let Some(last) = tracks.last_mut()
                && last.end.is_none()
            {
                last.end = total_secs;
            }
            return Some(tracks);
        }
    }

    read_binary_cuesheet_block(path, total_secs)
}

/// Look up the raw `CUESHEET` Vorbis comment (case-insensitive key match,
/// matching `ExtractCuesheetTagHandler::OnPair`'s
/// `StringIsEqualIgnoreCase(name, "cuesheet")` in
/// `EmbeddedCuePlaylistPlugin.cxx`).
fn read_cuesheet_comment(path: &Utf8PathBuf) -> Option<String> {
    let pairs = MetadataExtractor::read_raw_comments(path).ok()?;
    pairs
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cuesheet"))
        .map(|(_, v)| v)
        .filter(|v| !v.trim().is_empty())
}

/// Decode the binary `CUESHEET` metadata block (via Symphonia's FLAC
/// demuxer) into synthetic [`CueTrack`]s carrying only start/end times and a
/// sequential (1-based) track number — the block has no title/performer
/// fields, and Symphonia's `Chapter` doesn't preserve the on-disk track
/// number either (only an ISRC tag), so position stands in for it.
///
/// A CD-DA cuesheet's chapter tree nests per-index chapters inside a group
/// per track when the track has `INDEX` points (mirroring a CD's INDEX00
/// pregap + INDEX01 start); the track's audible start is its *last* index
/// (INDEX01, when present), matching `parse_cue`'s `index01.or(index00)`.
fn read_binary_cuesheet_block(
    path: &Utf8PathBuf,
    total_secs: Option<f64>,
) -> Option<Vec<CueTrack>> {
    let file = std::fs::File::open(path.as_str()).ok()?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(ext);
    }
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .ok()?;
    let group = reader.chapters()?;
    if group.items.is_empty() {
        return None;
    }

    let mut starts: Vec<f64> = Vec::with_capacity(group.items.len());
    for item in &group.items {
        let start = match item {
            ChapterGroupItem::Chapter(c) => c.start_time.as_secs_f64(),
            ChapterGroupItem::Group(g) => g.items.iter().rev().find_map(|gi| match gi {
                ChapterGroupItem::Chapter(c) => Some(c.start_time.as_secs_f64()),
                ChapterGroupItem::Group(_) => None,
            })?,
        };
        starts.push(start);
    }

    // Drop a trailing CD-DA lead-out marker (see `LEAD_OUT_TOLERANCE_SECS`).
    if starts.len() > 1
        && matches!(group.items.last(), Some(ChapterGroupItem::Chapter(_)))
        && let (Some(&last_start), Some(total)) = (starts.last(), total_secs)
        && (total - last_start).abs() < LEAD_OUT_TOLERANCE_SECS
    {
        starts.pop();
    }

    if starts.is_empty() {
        return None;
    }

    let file_name = path.file_name().unwrap_or_default().to_string();
    Some(
        starts
            .iter()
            .enumerate()
            .map(|(i, &start)| CueTrack {
                file: file_name.clone(),
                number: (i + 1) as u32,
                title: None,
                performer: None,
                album: None,
                album_performer: None,
                start,
                end: starts.get(i + 1).copied().or(total_secs),
            })
            .collect(),
    )
}

/// Build the per-track virtual [`rmpd_core::song::Song`] rows for `container`
/// (a normally-scanned FLAC `Song`, already carrying its own tags/audio
/// properties) from its parsed cue tracks. Each track's `path` is
/// `<container path>/track{NNNN}` (matching MPD's `UpdatePlaylistFile`
/// naming for virtual-directory playlist entries), its audio properties and
/// ReplayGain are inherited from `container` (same physical stream), and its
/// tags start from `container`'s own tags (so genre/date/composer/... carry
/// over) with title/artist/album/albumartist/track overridden per-track:
/// from the cue sheet when it specifies them (only the text form does),
/// else combining `container`'s own TITLE tag with the track number, or a
/// bare "Track N" when it has none.
pub fn build_container_tracks(
    container: &rmpd_core::song::Song,
    cue_tracks: &[CueTrack],
) -> Vec<rmpd_core::song::Song> {
    use rmpd_core::song::{Song, intern_tag_key};

    let container_secs = container.duration.map(|d| d.as_secs_f64());
    let base_title = container.tag("title").map(str::to_string);
    let base_artist = container.tag("artist").map(str::to_string);
    let base_album = container.tag("album").map(str::to_string);
    let base_album_artist = container.tag("albumartist").map(str::to_string);

    cue_tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let start = t.start.max(0.0);
            let end = t.end.or(container_secs).filter(|&e| e > start);
            // `try_from_secs_f64` (not `from_secs_f64`, which panics) so a
            // malformed/adversarial cue timecode (e.g. an absurd `MM` far
            // beyond `Duration::MAX`) degrades to "no duration" instead of
            // crashing the scanner on untrusted file content.
            let duration = end.and_then(|e| Duration::try_from_secs_f64(e - start).ok());

            let mut tags: Vec<_> = container
                .tags
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_ref(),
                        "title" | "artist" | "album" | "albumartist" | "track"
                    )
                })
                .cloned()
                .collect();

            // Per-track TITLE: the cue text's own value when it has one
            // (only the text form does); otherwise "derived from file tags
            // + track number" — the container's own TITLE tag combined with
            // the track number stays distinguishable per track, falling
            // back to a bare "Track N" when the container has no title tag
            // at all (the common case for a binary-cuesheet-only rip).
            let title = match &t.title {
                Some(title) => title.clone(),
                None => match &base_title {
                    Some(base) => format!("{base} - Track {}", t.number),
                    None => format!("Track {}", t.number),
                },
            };
            tags.push((intern_tag_key("title"), title));

            if let Some(artist) = t.performer.clone().or_else(|| base_artist.clone()) {
                tags.push((intern_tag_key("artist"), artist));
            }
            if let Some(album) = t.album.clone().or_else(|| base_album.clone()) {
                tags.push((intern_tag_key("album"), album));
            }
            if let Some(aa) = t
                .album_performer
                .clone()
                .or_else(|| base_album_artist.clone())
            {
                tags.push((intern_tag_key("albumartist"), aa));
            }
            tags.push((intern_tag_key("track"), t.number.to_string()));

            Song {
                id: 0,
                path: container.path.join(format!("track{:04}", i + 1)),
                duration,
                sample_rate: container.sample_rate,
                channels: container.channels,
                bits_per_sample: container.bits_per_sample,
                bitrate: container.bitrate,
                replay_gain_track_gain: container.replay_gain_track_gain,
                replay_gain_track_peak: container.replay_gain_track_peak,
                replay_gain_album_gain: container.replay_gain_album_gain,
                replay_gain_album_peak: container.replay_gain_album_peak,
                added_at: container.added_at,
                last_modified: container.last_modified,
                range: Some((start, end.unwrap_or(start))),
                tags,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::{Song, intern_tag_key};

    fn container_song() -> Song {
        Song {
            id: 0,
            path: "Artist/Album.flac".into(),
            duration: Some(Duration::from_secs(120)),
            sample_rate: Some(44100),
            channels: Some(2),
            bits_per_sample: Some(16),
            bitrate: Some(1000),
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 42,
            last_modified: 42,
            range: None,
            tags: vec![
                (intern_tag_key("album"), "Album".to_string()),
                (intern_tag_key("genre"), "Rock".to_string()),
            ],
        }
    }

    #[test]
    fn container_track_paths_are_sequential_and_range_restricted() {
        let cue = "\
FILE \"whatever.wav\" WAVE
  TRACK 01 AUDIO
    TITLE \"One\"
    PERFORMER \"Artist\"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE \"Two\"
    INDEX 01 01:00:00
";
        let tracks = parse_cue(cue);
        let songs = build_container_tracks(&container_song(), &tracks);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].path.as_str(), "Artist/Album.flac/track0001");
        assert_eq!(songs[1].path.as_str(), "Artist/Album.flac/track0002");
        assert_eq!(songs[0].range, Some((0.0, 60.0)));
        assert_eq!(songs[1].range, Some((60.0, 120.0)));
        assert_eq!(songs[0].tag("title"), Some("One"));
        assert_eq!(songs[0].tag("artist"), Some("Artist"));
        assert_eq!(songs[1].tag("title"), Some("Two"));
        // Missing per-track album falls back to the container's own tag.
        assert_eq!(songs[1].tag("album"), Some("Album"));
        assert_eq!(songs[1].tag("genre"), Some("Rock"));
        assert_eq!(songs[1].tag("track"), Some("2"));
    }

    #[test]
    fn missing_title_falls_back_to_synthesized_track_name() {
        let cue = "\
FILE \"whatever.wav\" WAVE
  TRACK 05 AUDIO
    INDEX 01 00:00:00
";
        let tracks = parse_cue(cue);
        let songs = build_container_tracks(&container_song(), &tracks);
        assert_eq!(songs[0].tag("title"), Some("Track 5"));
    }

    /// Deterministic mutation fuzz: byte-flip a well-formed CUESHEET text at
    /// every offset and feed the result through the full
    /// `parse_cue` → `build_container_tracks` pipeline. Malformed/adversarial
    /// cue text (huge timecodes, truncated lines, garbage bytes) must never
    /// panic -- `parse_cue` already returns partial/empty results for
    /// nonsense input, and `build_container_tracks` guards the one panicking
    /// primitive it calls (`Duration::from_secs_f64`) with
    /// `try_from_secs_f64` instead.
    #[test]
    fn mutated_cue_text_never_panics() {
        let base = "TITLE \"Album\"\nFILE \"x.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 99999999999999:59:74\n";
        let container = container_song();
        for i in 0..base.len() {
            for &repl in &[b'\0', b'9', b':', b'"', b'\n', 0xFFu8] {
                let mut bytes = base.as_bytes().to_vec();
                bytes[i] = repl;
                let mutated = String::from_utf8_lossy(&bytes).into_owned();
                let tracks = parse_cue(&mutated);
                let _songs = build_container_tracks(&container, &tracks);
            }
        }
    }
}
