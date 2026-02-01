/// Comparison utilities for verifying rmpd behavior matches MPD
///
/// This module provides tiered comparison strategies for different types
/// of metadata and behavioral outputs.

use rmpd_core::song::Song;

/// Tolerance levels for comparing metadata between rmpd and MPD
#[derive(Debug, Clone, Copy)]
pub enum ComparisonTolerance {
    /// Exact match required (for IDs, core tags)
    Exact,
    /// Fuzzy match allowed (for duration, bitrate with thresholds)
    Fuzzy,
    /// Presence check only (artwork exists, comments present)
    Presence,
}

/// Configuration for metadata comparison with tolerances
#[derive(Debug, Clone)]
pub struct ComparisonConfig {
    /// Duration tolerance in seconds (±1s is typical)
    pub duration_tolerance_secs: f64,
    /// Bitrate tolerance as percentage (±10% is typical)
    pub bitrate_tolerance_percent: f64,
    /// Whether to compare date format strictly
    pub strict_date_format: bool,
    /// Whether to require exact tag casing
    pub case_sensitive_tags: bool,
}

impl Default for ComparisonConfig {
    fn default() -> Self {
        Self {
            duration_tolerance_secs: 1.0,
            bitrate_tolerance_percent: 10.0,
            strict_date_format: false,
            case_sensitive_tags: false,
        }
    }
}

/// Result of a metadata comparison
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparisonResult {
    /// Perfect match
    Exact,
    /// Match within tolerance
    Acceptable(String),
    /// Mismatch beyond tolerance
    Mismatch(String),
}

impl ComparisonResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, ComparisonResult::Exact | ComparisonResult::Acceptable(_))
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            ComparisonResult::Exact => None,
            ComparisonResult::Acceptable(msg) | ComparisonResult::Mismatch(msg) => Some(msg),
        }
    }
}

/// Compare two songs with the specified configuration
pub fn compare_songs(
    rmpd_song: &Song,
    mpd_song: &Song,
    config: &ComparisonConfig,
) -> Vec<(String, ComparisonResult)> {
    let mut results = Vec::new();

    // Level 1: Exact comparisons (core tags, MusicBrainz IDs)
    results.push((
        "title".to_string(),
        compare_option_exact(&rmpd_song.title, &mpd_song.title),
    ));
    results.push((
        "artist".to_string(),
        compare_option_exact(&rmpd_song.artist, &mpd_song.artist),
    ));
    results.push((
        "album".to_string(),
        compare_option_exact(&rmpd_song.album, &mpd_song.album),
    ));
    results.push((
        "album_artist".to_string(),
        compare_option_exact(&rmpd_song.album_artist, &mpd_song.album_artist),
    ));
    results.push((
        "genre".to_string(),
        compare_option_exact(&rmpd_song.genre, &mpd_song.genre),
    ));

    // MusicBrainz IDs (should be exact)
    results.push((
        "musicbrainz_trackid".to_string(),
        compare_option_exact(&rmpd_song.musicbrainz_trackid, &mpd_song.musicbrainz_trackid),
    ));
    results.push((
        "musicbrainz_albumid".to_string(),
        compare_option_exact(&rmpd_song.musicbrainz_albumid, &mpd_song.musicbrainz_albumid),
    ));

    // Level 2: Fuzzy comparisons (duration, bitrate, dates)
    results.push((
        "duration".to_string(),
        compare_duration(
            rmpd_song.duration,
            mpd_song.duration,
            config.duration_tolerance_secs,
        ),
    ));
    results.push((
        "bitrate".to_string(),
        compare_bitrate(
            rmpd_song.bitrate,
            mpd_song.bitrate,
            config.bitrate_tolerance_percent,
        ),
    ));
    results.push((
        "date".to_string(),
        compare_date(&rmpd_song.date, &mpd_song.date, config.strict_date_format),
    ));

    // Level 3: Presence checks (ReplayGain, comments)
    results.push((
        "replay_gain_track_gain".to_string(),
        compare_presence(
            rmpd_song.replay_gain_track_gain.is_some(),
            mpd_song.replay_gain_track_gain.is_some(),
        ),
    ));
    results.push((
        "comment".to_string(),
        compare_presence(
            rmpd_song.comment.is_some(),
            mpd_song.comment.is_some(),
        ),
    ));

    // Audio properties
    results.push((
        "sample_rate".to_string(),
        compare_option_exact(&rmpd_song.sample_rate, &mpd_song.sample_rate),
    ));
    results.push((
        "channels".to_string(),
        compare_option_exact(&rmpd_song.channels, &mpd_song.channels),
    ));

    results
}

fn compare_option_exact<T: PartialEq + std::fmt::Debug>(
    a: &Option<T>,
    b: &Option<T>,
) -> ComparisonResult {
    match (a, b) {
        (Some(a_val), Some(b_val)) if a_val == b_val => ComparisonResult::Exact,
        (None, None) => ComparisonResult::Exact,
        (Some(a_val), Some(b_val)) => {
            ComparisonResult::Mismatch(format!("Expected {:?}, got {:?}", b_val, a_val))
        }
        (Some(a_val), None) => {
            ComparisonResult::Mismatch(format!("rmpd has value {:?}, MPD has None", a_val))
        }
        (None, Some(b_val)) => {
            ComparisonResult::Mismatch(format!("rmpd has None, MPD has {:?}", b_val))
        }
    }
}

fn compare_duration(
    rmpd_duration: Option<std::time::Duration>,
    mpd_duration: Option<std::time::Duration>,
    tolerance_secs: f64,
) -> ComparisonResult {
    match (rmpd_duration, mpd_duration) {
        (Some(a), Some(b)) => {
            let diff = (a.as_secs_f64() - b.as_secs_f64()).abs();
            if diff < 0.001 {
                ComparisonResult::Exact
            } else if diff <= tolerance_secs {
                ComparisonResult::Acceptable(format!(
                    "Duration differs by {:.3}s (within ±{}s tolerance)",
                    diff, tolerance_secs
                ))
            } else {
                ComparisonResult::Mismatch(format!(
                    "Duration differs by {:.3}s (exceeds ±{}s tolerance)",
                    diff, tolerance_secs
                ))
            }
        }
        (None, None) => ComparisonResult::Exact,
        (Some(a), None) => {
            ComparisonResult::Mismatch(format!("rmpd has duration {:?}, MPD has None", a))
        }
        (None, Some(b)) => {
            ComparisonResult::Mismatch(format!("rmpd has None, MPD has duration {:?}", b))
        }
    }
}

fn compare_bitrate(
    rmpd_bitrate: Option<u32>,
    mpd_bitrate: Option<u32>,
    tolerance_percent: f64,
) -> ComparisonResult {
    match (rmpd_bitrate, mpd_bitrate) {
        (Some(a), Some(b)) => {
            if a == b {
                ComparisonResult::Exact
            } else {
                let diff_percent = ((a as f64 - b as f64).abs() / b as f64) * 100.0;
                if diff_percent <= tolerance_percent {
                    ComparisonResult::Acceptable(format!(
                        "Bitrate differs by {:.1}% (within ±{}% tolerance)",
                        diff_percent, tolerance_percent
                    ))
                } else {
                    ComparisonResult::Mismatch(format!(
                        "Bitrate differs by {:.1}% (exceeds ±{}% tolerance)",
                        diff_percent, tolerance_percent
                    ))
                }
            }
        }
        (None, None) => ComparisonResult::Exact,
        (Some(a), None) => {
            ComparisonResult::Mismatch(format!("rmpd has bitrate {}, MPD has None", a))
        }
        (None, Some(b)) => {
            ComparisonResult::Mismatch(format!("rmpd has None, MPD has bitrate {}", b))
        }
    }
}

fn compare_date(
    rmpd_date: &Option<String>,
    mpd_date: &Option<String>,
    strict: bool,
) -> ComparisonResult {
    match (rmpd_date, mpd_date) {
        (Some(a), Some(b)) if a == b => ComparisonResult::Exact,
        (Some(a), Some(b)) if !strict => {
            // Try to extract year and compare
            let a_year = extract_year(a);
            let b_year = extract_year(b);
            if a_year == b_year {
                ComparisonResult::Acceptable(format!(
                    "Date format differs (\"{a}\" vs \"{b}\") but year matches"
                ))
            } else {
                ComparisonResult::Mismatch(format!(
                    "Date year mismatch: \"{}\" vs \"{}\"",
                    a, b
                ))
            }
        }
        (None, None) => ComparisonResult::Exact,
        (Some(a), Some(b)) => {
            ComparisonResult::Mismatch(format!("Date mismatch: \"{}\" vs \"{}\"", a, b))
        }
        (Some(a), None) => {
            ComparisonResult::Mismatch(format!("rmpd has date \"{}\", MPD has None", a))
        }
        (None, Some(b)) => {
            ComparisonResult::Mismatch(format!("rmpd has None, MPD has date \"{}\"", b))
        }
    }
}

fn extract_year(date: &str) -> Option<u32> {
    // Try to extract 4-digit year from various formats
    date.chars()
        .collect::<String>()
        .split(|c: char| !c.is_numeric())
        .filter_map(|s| s.parse::<u32>().ok())
        .find(|&y| y >= 1000 && y <= 9999)
}

fn compare_presence(rmpd_present: bool, mpd_present: bool) -> ComparisonResult {
    match (rmpd_present, mpd_present) {
        (true, true) | (false, false) => ComparisonResult::Exact,
        (true, false) => ComparisonResult::Acceptable("rmpd has value, MPD doesn't".to_string()),
        (false, true) => ComparisonResult::Acceptable("MPD has value, rmpd doesn't".to_string()),
    }
}

/// Assert that all comparison results are acceptable
pub fn assert_songs_match(
    rmpd_song: &Song,
    mpd_song: &Song,
    config: &ComparisonConfig,
) {
    let results = compare_songs(rmpd_song, mpd_song, config);

    let mut failures = Vec::new();
    for (field, result) in &results {
        if !result.is_ok() {
            if let Some(msg) = result.message() {
                failures.push(format!("  {}: {}", field, msg));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "Song metadata comparison failed:\n{}\n\nrmpd song: {:?}\n\nMPD song: {:?}",
            failures.join("\n"),
            rmpd_song,
            mpd_song
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::Song;
    use std::time::Duration;

    fn create_test_song() -> Song {
        Song {
            id: 1,
            path: "/music/test.mp3".into(),
            duration: Some(Duration::from_secs(180)),
            title: Some("Test Song".to_string()),
            artist: Some("Test Artist".to_string()),
            album: Some("Test Album".to_string()),
            album_artist: None,
            track: Some(1),
            disc: None,
            date: Some("2024".to_string()),
            genre: Some("Rock".to_string()),
            composer: None,
            performer: None,
            comment: None,
            musicbrainz_trackid: Some("12345678-1234-1234-1234-123456789012".to_string()),
            musicbrainz_albumid: None,
            musicbrainz_artistid: None,
            musicbrainz_albumartistid: None,
            musicbrainz_releasegroupid: None,
            musicbrainz_releasetrackid: None,
            artist_sort: None,
            album_artist_sort: None,
            original_date: None,
            label: None,
            sample_rate: Some(44100),
            channels: Some(2),
            bits_per_sample: Some(16),
            bitrate: Some(320),
            replay_gain_track_gain: Some(-6.0),
            replay_gain_track_peak: Some(0.95),
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
        }
    }

    #[test]
    fn test_exact_match() {
        let song1 = create_test_song();
        let song2 = song1.clone();

        let config = ComparisonConfig::default();
        let results = compare_songs(&song1, &song2, &config);

        for (_field, result) in results {
            assert!(result.is_ok(), "Expected all fields to match exactly");
        }
    }

    #[test]
    fn test_duration_tolerance() {
        let mut song1 = create_test_song();
        let mut song2 = create_test_song();

        song1.duration = Some(Duration::from_millis(180_500)); // 180.5s
        song2.duration = Some(Duration::from_millis(181_200)); // 181.2s

        let result = compare_duration(song1.duration, song2.duration, 1.0);
        assert!(result.is_ok(), "Should be within 1 second tolerance");
    }

    #[test]
    fn test_bitrate_tolerance() {
        let result = compare_bitrate(Some(320), Some(315), 10.0);
        assert!(
            result.is_ok(),
            "Should be within 10% tolerance (1.6% difference)"
        );
    }

    #[test]
    fn test_date_format_flexibility() {
        let result = compare_date(&Some("2024".to_string()), &Some("2024-01-15".to_string()), false);
        assert!(result.is_ok(), "Should match year even with different formats");
    }

    #[test]
    fn test_extract_year() {
        assert_eq!(extract_year("2024"), Some(2024));
        assert_eq!(extract_year("2024-01-15"), Some(2024));
        assert_eq!(extract_year("15/01/2024"), Some(2024));
        assert_eq!(extract_year("invalid"), None);
    }
}
