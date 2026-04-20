//! Shared test utilities for rmpd workspace
//!
//! This module provides common test helpers and fixtures used across
//! multiple test suites in the workspace. Only available when the
//! "test-utils" feature is enabled.

use crate::song::{Song, intern_tag_key};
use camino::Utf8PathBuf;
use std::path::PathBuf;
use std::time::Duration;

// ── Song creation helpers ────────────────────────────────────────────

/// Create a test song with minimal required fields
///
/// # Examples
///
/// ```
/// # use rmpd_core::test_utils::create_test_song;
/// let song = create_test_song(1, "test");
/// assert_eq!(song.id, 1);
/// assert_eq!(song.tag("title"), Some("Song test"));
/// ```
pub fn create_test_song(id: u64, name: &str) -> Song {
    Song {
        id,
        path: Utf8PathBuf::from(format!("song{}.mp3", name)),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![(intern_tag_key("title"), format!("Song {}", name))],
    }
}

/// Create a test song with custom metadata fields
///
/// # Examples
///
/// ```
/// # use rmpd_core::test_utils::create_test_song_with_metadata;
/// let song = create_test_song_with_metadata(
///     1,
///     "test.mp3",
///     Some("Test Title"),
///     Some("Test Artist"),
///     Some("Test Album"),
/// );
/// assert_eq!(song.tag("title"), Some("Test Title"));
/// assert_eq!(song.tag("artist"), Some("Test Artist"));
/// ```
pub fn create_test_song_with_metadata(
    id: u64,
    path: &str,
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
) -> Song {
    let mut tags = Vec::new();
    if let Some(v) = title {
        tags.push((intern_tag_key("title"), v.to_string()));
    }
    if let Some(v) = artist {
        tags.push((intern_tag_key("artist"), v.to_string()));
    }
    if let Some(v) = album {
        tags.push((intern_tag_key("album"), v.to_string()));
    }
    Song {
        id,
        path: Utf8PathBuf::from(path),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags,
    }
}

/// Create a fully-populated test song with the given path and track number.
///
/// Returns a song with duration, sample rate, channels, bitrate, and standard
/// tags (title, artist, album, track, date, genre). Used by protocol and
/// library integration tests that need realistic song data.
pub fn make_test_song(path: &str, track: u32) -> Song {
    Song {
        id: track as u64,
        path: path.into(),
        duration: Some(Duration::from_secs(180)),
        sample_rate: Some(44100),
        channels: Some(2),
        bits_per_sample: Some(16),
        bitrate: Some(320),
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: vec![
            (intern_tag_key("title"), format!("Track {track}")),
            (intern_tag_key("artist"), "Test Artist".to_string()),
            (intern_tag_key("album"), "Test Album".to_string()),
            (intern_tag_key("track"), track.to_string()),
            (intern_tag_key("date"), "2024".to_string()),
            (intern_tag_key("genre"), "Rock".to_string()),
        ],
    }
}

// ── Fixture utilities ────────────────────────────────────────────────

/// Audio format for test fixture generation (FFmpeg-based).
///
/// Shared across player and library fixture generators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Flac,
    Mp3,
    Ogg,
    Opus,
    M4a,
    Wav,
}

impl AudioFormat {
    /// File extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Opus => "opus",
            AudioFormat::M4a => "m4a",
            AudioFormat::Wav => "wav",
        }
    }

    /// FFmpeg codec name for this format.
    pub fn codec(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "libmp3lame",
            AudioFormat::Ogg => "libvorbis",
            AudioFormat::Opus => "libopus",
            AudioFormat::M4a => "aac",
            AudioFormat::Wav => "pcm_s16le",
        }
    }
}

/// Sanitize a string for safe use in filenames.
///
/// Replaces filesystem-unsafe characters and spaces with underscores.
pub fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '_',
            c => c,
        })
        .collect()
}

/// Get the fixtures sample directory for a given crate.
///
/// `manifest_dir` should be `env!("CARGO_MANIFEST_DIR")` from the calling crate.
pub fn fixtures_dir(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir).join("tests/fixtures/samples")
}

/// Get a specific fixture file path.
///
/// `manifest_dir` should be `env!("CARGO_MANIFEST_DIR")` from the calling crate.
pub fn get_fixture(manifest_dir: &str, filename: &str) -> PathBuf {
    fixtures_dir(manifest_dir).join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_song() {
        let song = create_test_song(42, "test");
        assert_eq!(song.id, 42);
        assert_eq!(song.tag("title"), Some("Song test"));
        assert_eq!(song.path.as_str(), "songtest.mp3");
    }

    #[test]
    fn test_create_test_song_with_metadata() {
        let song = create_test_song_with_metadata(
            1,
            "test.mp3",
            Some("My Title"),
            Some("My Artist"),
            Some("My Album"),
        );
        assert_eq!(song.id, 1);
        assert_eq!(song.tag("title"), Some("My Title"));
        assert_eq!(song.tag("artist"), Some("My Artist"));
        assert_eq!(song.tag("album"), Some("My Album"));
    }

    #[test]
    fn test_make_test_song() {
        let song = make_test_song("/music/test.flac", 5);
        assert_eq!(song.id, 5);
        assert_eq!(song.path.as_str(), "/music/test.flac");
        assert_eq!(song.tag("title"), Some("Track 5"));
        assert_eq!(song.tag("artist"), Some("Test Artist"));
        assert_eq!(song.tag("album"), Some("Test Album"));
        assert_eq!(song.tag("track"), Some("5"));
        assert_eq!(song.tag("date"), Some("2024"));
        assert_eq!(song.tag("genre"), Some("Rock"));
        assert_eq!(song.duration, Some(Duration::from_secs(180)));
        assert_eq!(song.sample_rate, Some(44100));
        assert_eq!(song.channels, Some(2));
        assert_eq!(song.bitrate, Some(320));
    }

    #[test]
    fn test_audio_format() {
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Ogg.extension(), "ogg");
        assert_eq!(AudioFormat::Opus.extension(), "opus");
        assert_eq!(AudioFormat::M4a.extension(), "m4a");
        assert_eq!(AudioFormat::Wav.extension(), "wav");

        assert_eq!(AudioFormat::Flac.codec(), "flac");
        assert_eq!(AudioFormat::Mp3.codec(), "libmp3lame");
        assert_eq!(AudioFormat::Opus.codec(), "libopus");
    }

    #[test]
    fn test_sanitize_for_filename() {
        assert_eq!(sanitize_for_filename("sine/440hz:test"), "sine_440hz_test");
        assert_eq!(sanitize_for_filename("hello world"), "hello_world");
        assert_eq!(sanitize_for_filename("a*b?c"), "a_b_c");
    }

    #[test]
    fn test_fixtures_dir() {
        let dir = fixtures_dir("/some/crate");
        assert_eq!(dir, PathBuf::from("/some/crate/tests/fixtures/samples"));
    }

    #[test]
    fn test_get_fixture() {
        let path = get_fixture("/some/crate", "test.flac");
        assert_eq!(
            path,
            PathBuf::from("/some/crate/tests/fixtures/samples/test.flac")
        );
    }
}
