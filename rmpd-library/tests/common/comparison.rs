/// Comparison utilities for verifying rmpd behavior matches MPD
///
/// This module provides tiered comparison strategies for different types
/// of metadata and behavioral outputs.
use rmpd_core::song::Song;

/// Configuration for metadata comparison with tolerances
#[derive(Debug, Clone)]
pub struct ComparisonConfig {
    /// Duration tolerance in seconds (±1s is typical)
    pub duration_tolerance_secs: f64,
    /// Bitrate tolerance as percentage (±10% is typical)
    pub bitrate_tolerance_percent: f64,
    /// Whether to compare date format strictly
    pub strict_date_format: bool,
}

impl Default for ComparisonConfig {
    fn default() -> Self {
        Self {
            duration_tolerance_secs: 1.0,
            bitrate_tolerance_percent: 10.0,
            strict_date_format: false,
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
        matches!(
            self,
            ComparisonResult::Exact | ComparisonResult::Acceptable(_)
        )
    }
}

/// Compare two songs with the specified configuration
pub fn compare_songs(
    rmpd_song: &Song,
    mpd_song: &Song,
    config: &ComparisonConfig,
) -> Vec<(String, ComparisonResult)> {
    let rmpd_title = rmpd_song.tag("title").map(|s| s.to_string());
    let mpd_title = mpd_song.tag("title").map(|s| s.to_string());
    let rmpd_artist = rmpd_song.tag("artist").map(|s| s.to_string());
    let mpd_artist = mpd_song.tag("artist").map(|s| s.to_string());
    let rmpd_album = rmpd_song.tag("album").map(|s| s.to_string());
    let mpd_album = mpd_song.tag("album").map(|s| s.to_string());
    let rmpd_album_artist = rmpd_song.tag("albumartist").map(|s| s.to_string());
    let mpd_album_artist = mpd_song.tag("albumartist").map(|s| s.to_string());
    let rmpd_genre = rmpd_song.tag("genre").map(|s| s.to_string());
    let mpd_genre = mpd_song.tag("genre").map(|s| s.to_string());
    let rmpd_mb_trackid = rmpd_song.tag("musicbrainz_trackid").map(|s| s.to_string());
    let mpd_mb_trackid = mpd_song.tag("musicbrainz_trackid").map(|s| s.to_string());
    let rmpd_mb_albumid = rmpd_song.tag("musicbrainz_albumid").map(|s| s.to_string());
    let mpd_mb_albumid = mpd_song.tag("musicbrainz_albumid").map(|s| s.to_string());
    let rmpd_date = rmpd_song.tag("date").map(|s| s.to_string());
    let mpd_date = mpd_song.tag("date").map(|s| s.to_string());

    vec![
        // Level 1: Exact comparisons (core tags, MusicBrainz IDs)
        (
            "title".to_string(),
            compare_option_exact(&rmpd_title, &mpd_title),
        ),
        (
            "artist".to_string(),
            compare_option_exact(&rmpd_artist, &mpd_artist),
        ),
        (
            "album".to_string(),
            compare_option_exact(&rmpd_album, &mpd_album),
        ),
        (
            "album_artist".to_string(),
            compare_option_exact(&rmpd_album_artist, &mpd_album_artist),
        ),
        (
            "genre".to_string(),
            compare_option_exact(&rmpd_genre, &mpd_genre),
        ),
        // MusicBrainz IDs (should be exact)
        (
            "musicbrainz_trackid".to_string(),
            compare_option_exact(&rmpd_mb_trackid, &mpd_mb_trackid),
        ),
        (
            "musicbrainz_albumid".to_string(),
            compare_option_exact(&rmpd_mb_albumid, &mpd_mb_albumid),
        ),
        // Level 2: Fuzzy comparisons (duration, bitrate, dates)
        (
            "duration".to_string(),
            compare_duration(
                rmpd_song.duration,
                mpd_song.duration,
                config.duration_tolerance_secs,
            ),
        ),
        (
            "bitrate".to_string(),
            compare_bitrate(
                rmpd_song.bitrate,
                mpd_song.bitrate,
                config.bitrate_tolerance_percent,
            ),
        ),
        (
            "date".to_string(),
            compare_date(&rmpd_date, &mpd_date, config.strict_date_format),
        ),
        // Level 3: Presence checks (ReplayGain, comments)
        (
            "replay_gain_track_gain".to_string(),
            compare_presence(
                rmpd_song.replay_gain_track_gain.is_some(),
                mpd_song.replay_gain_track_gain.is_some(),
            ),
        ),
        (
            "comment".to_string(),
            compare_presence(
                rmpd_song.tag("comment").is_some(),
                mpd_song.tag("comment").is_some(),
            ),
        ),
        // Audio properties
        (
            "sample_rate".to_string(),
            compare_option_exact(&rmpd_song.sample_rate, &mpd_song.sample_rate),
        ),
        (
            "channels".to_string(),
            compare_option_exact(&rmpd_song.channels, &mpd_song.channels),
        ),
    ]
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
                ComparisonResult::Mismatch(format!("Date year mismatch: \"{}\" vs \"{}\"", a, b))
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
        .find(|&y| (1000..=9999).contains(&y))
}

fn compare_presence(rmpd_present: bool, mpd_present: bool) -> ComparisonResult {
    match (rmpd_present, mpd_present) {
        (true, true) | (false, false) => ComparisonResult::Exact,
        (true, false) => ComparisonResult::Acceptable("rmpd has value, MPD doesn't".to_string()),
        (false, true) => ComparisonResult::Acceptable("MPD has value, rmpd doesn't".to_string()),
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
            tags: vec![
                ("title".to_string(), "Test Song".to_string()),
                ("artist".to_string(), "Test Artist".to_string()),
                ("album".to_string(), "Test Album".to_string()),
                ("track".to_string(), "1".to_string()),
                ("date".to_string(), "2024".to_string()),
                ("genre".to_string(), "Rock".to_string()),
                (
                    "musicbrainz_trackid".to_string(),
                    "12345678-1234-1234-1234-123456789012".to_string(),
                ),
            ],
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
        let result = compare_date(
            &Some("2024".to_string()),
            &Some("2024-01-15".to_string()),
            false,
        );
        assert!(
            result.is_ok(),
            "Should match year even with different formats"
        );
    }

    #[test]
    fn test_extract_year() {
        assert_eq!(extract_year("2024"), Some(2024));
        assert_eq!(extract_year("2024-01-15"), Some(2024));
        assert_eq!(extract_year("15/01/2024"), Some(2024));
        assert_eq!(extract_year("invalid"), None);
    }
}
