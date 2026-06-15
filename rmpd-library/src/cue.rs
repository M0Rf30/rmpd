//! CUE sheet parser.
//!
//! A `.cue` sheet describes how one or more audio files are divided into
//! individual tracks. Each `FILE` references an audio file; each `TRACK` within
//! it carries an `INDEX 01` start position (the audible track start). A track
//! plays from its own `INDEX 01` up to the next track's `INDEX 01` in the same
//! file (or to the end of the file for the last track).
//!
//! This module turns a CUE sheet into a flat list of [`CueTrack`]s with
//! resolved start/end times and tags, suitable for adding to the play queue as
//! range-restricted virtual songs (mirroring MPD's `cuesheet` behaviour).

/// Frames per second in CUE `MM:SS:FF` timecodes (CD audio uses 75).
const CUE_FRAMES_PER_SECOND: f64 = 75.0;

/// A single virtual track extracted from a CUE sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct CueTrack {
    /// Audio file referenced by the enclosing `FILE` line (verbatim name).
    pub file: String,
    /// 1-based track number from the `TRACK` line.
    pub number: u32,
    /// Track `TITLE`, if any.
    pub title: Option<String>,
    /// Track `PERFORMER` (maps to the artist tag), if any.
    pub performer: Option<String>,
    /// Disc-level `TITLE` (maps to the album tag), if any.
    pub album: Option<String>,
    /// Disc-level `PERFORMER` (maps to the album-artist tag), if any.
    pub album_performer: Option<String>,
    /// Start time in seconds (from `INDEX 01`, falling back to `INDEX 00`).
    pub start: f64,
    /// End time in seconds. `None` means "until the end of the file" (the last
    /// track of a file).
    pub end: Option<f64>,
}

/// Parse a CUE timecode of the form `MM:SS:FF` into seconds.
/// Returns `None` for malformed input.
fn parse_timecode(s: &str) -> Option<f64> {
    let mut parts = s.split(':');
    let minutes: u64 = parts.next()?.trim().parse().ok()?;
    let seconds: u64 = parts.next()?.trim().parse().ok()?;
    let frames: u64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() || seconds >= 60 || frames >= 75 {
        return None;
    }
    Some(minutes as f64 * 60.0 + seconds as f64 + frames as f64 / CUE_FRAMES_PER_SECOND)
}

/// Split a CUE line into its leading keyword and the remaining argument text.
fn split_keyword(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (kw, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((kw, rest)) => (kw, rest.trim()),
        None => (trimmed, ""),
    };
    Some((kw.to_ascii_uppercase(), rest))
}

/// Strip surrounding double quotes from a CUE argument, if present; otherwise
/// take the first whitespace-delimited token.
fn unquote(arg: &str) -> String {
    let arg = arg.trim();
    if let Some(inner) = arg.strip_prefix('"') {
        if let Some(end) = inner.find('"') {
            return inner[..end].to_string();
        }
        return inner.to_string();
    }
    arg.split_whitespace().next().unwrap_or("").to_string()
}

#[derive(Default)]
struct PartialTrack {
    file: String,
    number: u32,
    title: Option<String>,
    performer: Option<String>,
    index00: Option<f64>,
    index01: Option<f64>,
}

/// Parse a CUE sheet's text into a list of virtual tracks.
///
/// Track end times are computed from the next track's start within the same
/// file; the final track of each file has `end == None`. Disc-level `TITLE` and
/// `PERFORMER` (before the first `TRACK`) become each track's `album` and
/// `album_performer`.
#[must_use]
pub fn parse_cue(content: &str) -> Vec<CueTrack> {
    let mut album: Option<String> = None;
    let mut album_performer: Option<String> = None;
    let mut current_file: Option<String> = None;
    let mut partials: Vec<PartialTrack> = Vec::new();
    let mut seen_track = false;

    for line in content.lines() {
        let Some((kw, rest)) = split_keyword(line) else {
            continue;
        };
        match kw.as_str() {
            "FILE" => {
                current_file = Some(unquote(rest));
            }
            "TRACK" => {
                seen_track = true;
                let number = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
                partials.push(PartialTrack {
                    file: current_file.clone().unwrap_or_default(),
                    number,
                    ..Default::default()
                });
            }
            "TITLE" => {
                let value = unquote(rest);
                if let Some(track) = partials.last_mut().filter(|_| seen_track) {
                    track.title = Some(value);
                } else {
                    album = Some(value);
                }
            }
            "PERFORMER" => {
                let value = unquote(rest);
                if let Some(track) = partials.last_mut().filter(|_| seen_track) {
                    track.performer = Some(value);
                } else {
                    album_performer = Some(value);
                }
            }
            "INDEX" => {
                let mut it = rest.split_whitespace();
                let index_num = it.next().and_then(|n| n.parse::<u32>().ok());
                let time = it.next().and_then(parse_timecode);
                if let (Some(idx), Some(t), Some(track)) = (index_num, time, partials.last_mut()) {
                    match idx {
                        0 => track.index00 = Some(t),
                        1 => track.index01 = Some(t),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    // Resolve start times and compute end from the next track in the same file.
    let starts: Vec<f64> = partials
        .iter()
        .map(|p| p.index01.or(p.index00).unwrap_or(0.0))
        .collect();

    partials
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let start = starts[i];
            // End = next track's start, but only if it lives in the same file.
            let end = partials
                .get(i + 1)
                .filter(|next| next.file == p.file)
                .map(|_| starts[i + 1]);
            CueTrack {
                file: p.file.clone(),
                number: p.number,
                title: p.title.clone(),
                performer: p.performer.clone(),
                album: album.clone(),
                album_performer: album_performer.clone(),
                start,
                end,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn parses_timecode() {
        assert!(approx(parse_timecode("00:00:00").unwrap(), 0.0));
        assert!(approx(parse_timecode("01:30:00").unwrap(), 90.0));
        // 37 frames = 37/75 s
        assert!(approx(
            parse_timecode("00:02:37").unwrap(),
            2.0 + 37.0 / 75.0
        ));
        assert_eq!(parse_timecode("00:60:00"), None); // seconds out of range
        assert_eq!(parse_timecode("00:00:75"), None); // frames out of range
        assert_eq!(parse_timecode("bad"), None);
    }

    #[test]
    fn single_file_multiple_tracks() {
        let cue = r#"
PERFORMER "The Band"
TITLE "Greatest Hits"
FILE "album.flac" WAVE
  TRACK 01 AUDIO
    TITLE "First"
    PERFORMER "The Band"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Second"
    INDEX 00 03:58:00
    INDEX 01 04:00:00
  TRACK 03 AUDIO
    TITLE "Third"
    INDEX 01 07:30:00
"#;
        let tracks = parse_cue(cue);
        assert_eq!(tracks.len(), 3);

        assert_eq!(tracks[0].number, 1);
        assert_eq!(tracks[0].title.as_deref(), Some("First"));
        assert_eq!(tracks[0].album.as_deref(), Some("Greatest Hits"));
        assert_eq!(tracks[0].album_performer.as_deref(), Some("The Band"));
        assert!(approx(tracks[0].start, 0.0));
        assert!(approx(tracks[0].end.unwrap(), 240.0)); // next INDEX 01 = 4:00

        // INDEX 01 (not INDEX 00 pregap) defines the audible start.
        assert!(approx(tracks[1].start, 240.0));
        assert!(approx(tracks[1].end.unwrap(), 450.0)); // 7:30

        // Last track in the file runs to EOF.
        assert_eq!(tracks[2].number, 3);
        assert_eq!(tracks[2].end, None);
    }

    #[test]
    fn multi_file_resets_end() {
        let cue = r#"
TITLE "Disc"
FILE "a.flac" WAVE
  TRACK 01 AUDIO
    INDEX 01 00:00:00
FILE "b.flac" WAVE
  TRACK 02 AUDIO
    INDEX 01 00:00:00
"#;
        let tracks = parse_cue(cue);
        assert_eq!(tracks.len(), 2);
        // Track 1 is the last (only) track of a.flac -> no end (different file).
        assert_eq!(tracks[0].file, "a.flac");
        assert_eq!(tracks[0].end, None);
        assert_eq!(tracks[1].file, "b.flac");
        assert_eq!(tracks[1].end, None);
    }

    #[test]
    fn track_performer_overrides_album_performer() {
        let cue = r#"
PERFORMER "Various"
TITLE "Comp"
FILE "x.flac" WAVE
  TRACK 01 AUDIO
    TITLE "Song"
    PERFORMER "Real Artist"
    INDEX 01 00:00:00
"#;
        let t = &parse_cue(cue)[0];
        assert_eq!(t.performer.as_deref(), Some("Real Artist"));
        assert_eq!(t.album_performer.as_deref(), Some("Various"));
        assert_eq!(t.album.as_deref(), Some("Comp"));
    }

    #[test]
    fn empty_or_garbage_yields_nothing() {
        assert!(parse_cue("").is_empty());
        assert!(parse_cue("not a cue sheet\njust text").is_empty());
    }
}
