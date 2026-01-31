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
/// assert_eq!(song.title, Some("Song test".to_string()));
/// ```
pub fn create_test_song(id: u64, name: &str) -> Song {
    Song {
        id,
        path: Utf8PathBuf::from(format!("song{}.mp3", name)),
        title: Some(format!("Song {}", name)),
        duration: None,
        artist: None,
        album: None,
        album_artist: None,
        track: None,
        disc: None,
        date: None,
        genre: None,
        composer: None,
        performer: None,
        comment: None,
        musicbrainz_trackid: None,
        musicbrainz_albumid: None,
        musicbrainz_artistid: None,
        musicbrainz_albumartistid: None,
        musicbrainz_releasegroupid: None,
        musicbrainz_releasetrackid: None,
        artist_sort: None,
        album_artist_sort: None,
        original_date: None,
        label: None,
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
/// assert_eq!(song.title, Some("Test Title".to_string()));
/// assert_eq!(song.artist, Some("Test Artist".to_string()));
/// ```
pub fn create_test_song_with_metadata(
    id: u64,
    path: &str,
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
) -> Song {
    Song {
        id,
        path: Utf8PathBuf::from(path),
        title: title.map(String::from),
        artist: artist.map(String::from),
        album: album.map(String::from),
        duration: None,
        album_artist: None,
        track: None,
        disc: None,
        date: None,
        genre: None,
        composer: None,
        performer: None,
        comment: None,
        musicbrainz_trackid: None,
        musicbrainz_albumid: None,
        musicbrainz_artistid: None,
        musicbrainz_albumartistid: None,
        musicbrainz_releasegroupid: None,
        musicbrainz_releasetrackid: None,
        artist_sort: None,
        album_artist_sort: None,
        original_date: None,
        label: None,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_song() {
        let song = create_test_song(42, "test");
        assert_eq!(song.id, 42);
        assert_eq!(song.title, Some("Song test".to_string()));
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
        assert_eq!(song.title, Some("My Title".to_string()));
        assert_eq!(song.artist, Some("My Artist".to_string()));
        assert_eq!(song.album, Some("My Album".to_string()));
    }
}
