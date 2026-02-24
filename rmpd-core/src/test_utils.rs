//! Shared test utilities for rmpd workspace
//!
//! This module provides common test helpers and fixtures used across
//! multiple test suites in the workspace. Only available when the
//! "test-utils" feature is enabled.

use crate::song::Song;
use camino::Utf8PathBuf;

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
        tags: vec![("title".to_string(), format!("Song {}", name))],
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
        tags.push(("title".to_string(), v.to_string()));
    }
    if let Some(v) = artist {
        tags.push(("artist".to_string(), v.to_string()));
    }
    if let Some(v) = album {
        tags.push(("album".to_string(), v.to_string()));
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
}
