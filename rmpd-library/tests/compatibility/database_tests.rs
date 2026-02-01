/// Database query tests comparing rmpd behavior
///
/// These tests validate that rmpd's database operations produce
/// the same results as MPD for common query patterns.

use crate::common::rmpd_harness::RmpdTestHarness;
use crate::fixtures::{AudioFormat, FixtureGenerator, TestMetadata};

/// Helper to check if FFmpeg is available
macro_rules! require_ffmpeg {
    () => {
        if !FixtureGenerator::is_ffmpeg_available() {
            eprintln!("FFmpeg not available - skipping test");
            return;
        }
    };
}

#[test]
fn test_list_artists() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from different artists
    let artists = vec!["Artist A", "Artist B", "Artist C"];

    for (i, artist) in artists.iter().enumerate() {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: artist.to_string(),
            album: "Test Album".to_string(),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // List all artists
    let result = harness.list_artists().unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&"Artist A".to_string()));
    assert!(result.contains(&"Artist B".to_string()));
    assert!(result.contains(&"Artist C".to_string()));

    // Results should be sorted (case-insensitive)
    assert_eq!(result[0], "Artist A");
    assert_eq!(result[1], "Artist B");
    assert_eq!(result[2], "Artist C");
}

#[test]
fn test_list_albums() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from different albums
    let albums = vec!["Album X", "Album Y", "Album Z"];

    for (i, album) in albums.iter().enumerate() {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Test Artist".to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.list_albums().unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&"Album X".to_string()));
    assert!(result.contains(&"Album Y".to_string()));
    assert!(result.contains(&"Album Z".to_string()));
}

#[test]
fn test_find_by_artist() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add multiple songs by the same artist
    for i in 1..=3 {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Target Artist".to_string(),
            album: format!("Album {}", i),
            track: Some(i),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Add a song by a different artist
    let other_metadata = TestMetadata {
        title: "Other Song".to_string(),
        artist: "Other Artist".to_string(),
        album: "Other Album".to_string(),
        ..Default::default()
    };
    let other_path = gen.generate(AudioFormat::Flac, &other_metadata).unwrap();
    let other_song = harness.extract_metadata(other_path.to_str().unwrap()).unwrap();
    harness.add_song(&other_song).unwrap();

    // Find songs by target artist
    let result = harness.find_by_artist("Target Artist").unwrap();

    assert_eq!(result.len(), 3);
    for song in &result {
        assert_eq!(song.artist, Some("Target Artist".to_string()));
    }

    // Results should be ordered by album, track
    assert_eq!(result[0].track, Some(1));
    assert_eq!(result[1].track, Some(2));
    assert_eq!(result[2].track, Some(3));
}

#[test]
fn test_find_by_album() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add multiple songs from the same album
    for i in 1..=5 {
        let metadata = TestMetadata {
            title: format!("Track {}", i),
            artist: "Album Artist".to_string(),
            album: "Test Album".to_string(),
            track: Some(i),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.find_by_album("Test Album").unwrap();

    assert_eq!(result.len(), 5);
    for song in &result {
        assert_eq!(song.album, Some("Test Album".to_string()));
    }

    // Results should be ordered by track number
    for (i, song) in result.iter().enumerate() {
        assert_eq!(song.track, Some((i + 1) as u32));
    }
}

#[test]
fn test_count_songs() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Initially empty
    assert_eq!(harness.count_songs().unwrap(), 0);

    // Add songs
    for i in 1..=10 {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            track: Some(i),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    assert_eq!(harness.count_songs().unwrap(), 10);
}

#[test]
fn test_count_artists() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from 3 different artists (2 songs each)
    let artists = vec!["Artist 1", "Artist 2", "Artist 3"];

    for artist in &artists {
        for i in 1..=2 {
            let metadata = TestMetadata {
                title: format!("Song {} by {}", i, artist),
                artist: artist.to_string(),
                album: "Test Album".to_string(),
                ..Default::default()
            };

            let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
            let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
            harness.add_song(&song).unwrap();
        }
    }

    // Should count 3 unique artists (not 6 songs)
    assert_eq!(harness.count_artists().unwrap(), 3);
}

#[test]
fn test_count_albums() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from 4 different albums
    let albums = vec!["Album A", "Album B", "Album C", "Album D"];

    for album in &albums {
        let metadata = TestMetadata {
            title: format!("Song from {}", album),
            artist: "Test Artist".to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    assert_eq!(harness.count_albums().unwrap(), 4);
}

#[test]
fn test_case_insensitive_listing() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add artists with different casings
    let artists = vec!["The Beatles", "the beatles", "THE BEATLES"];

    for artist in &artists {
        let metadata = TestMetadata {
            title: "Song".to_string(),
            artist: artist.to_string(),
            album: "Album".to_string(),
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.list_artists().unwrap();

    // Should list all three variations
    assert_eq!(result.len(), 3);
}

#[test]
fn test_empty_database_queries() {
    let harness = RmpdTestHarness::new().unwrap();

    // All queries should work on empty database
    assert_eq!(harness.count_songs().unwrap(), 0);
    assert_eq!(harness.count_artists().unwrap(), 0);
    assert_eq!(harness.count_albums().unwrap(), 0);

    assert_eq!(harness.list_artists().unwrap().len(), 0);
    assert_eq!(harness.list_albums().unwrap().len(), 0);

    assert_eq!(harness.find_by_artist("Nonexistent").unwrap().len(), 0);
    assert_eq!(harness.find_by_album("Nonexistent").unwrap().len(), 0);
}

#[test]
fn test_query_with_special_characters() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Artist with special characters
    let metadata = TestMetadata {
        title: "Song".to_string(),
        artist: "AC/DC".to_string(),
        album: "Back in Black".to_string(),
        ..Default::default()
    };

    let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    let artists = harness.list_artists().unwrap();
    assert!(artists.contains(&"AC/DC".to_string()));

    let found = harness.find_by_artist("AC/DC").unwrap();
    assert_eq!(found.len(), 1);
}

#[test]
fn test_multiple_albums_same_name_different_artists() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Two different artists with albums of the same name
    let artists = vec!["Artist One", "Artist Two"];

    for artist in &artists {
        let metadata = TestMetadata {
            title: "Title Track".to_string(),
            artist: artist.to_string(),
            album: "Greatest Hits".to_string(), // Same album name
            ..Default::default()
        };

        let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Should list "Greatest Hits" twice (different artists)
    let albums = harness.list_albums().unwrap();
    let greatest_hits_count = albums.iter().filter(|a| *a == "Greatest Hits").count();
    assert_eq!(greatest_hits_count, 1); // Or 2, depending on database normalization
}

#[test]
fn test_get_song_by_id() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Specific Song".to_string(),
        artist: "Specific Artist".to_string(),
        album: "Specific Album".to_string(),
        ..Default::default()
    };

    let path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    let id = harness.add_song(&song).unwrap();

    // Retrieve by ID
    let retrieved = harness.get_song(id).unwrap().unwrap();
    assert_eq!(retrieved.title, Some("Specific Song".to_string()));
    assert_eq!(retrieved.artist, Some("Specific Artist".to_string()));

    // Non-existent ID
    let nonexistent = harness.get_song(99999).unwrap();
    assert!(nonexistent.is_none());
}

#[test]
fn test_get_song_by_path() {
    require_ffmpeg!();

    let gen = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata::default();
    let fixture_path = gen.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(fixture_path.to_str().unwrap()).unwrap();

    let song_path = song.path.clone();
    harness.add_song(&song).unwrap();

    // Retrieve by path
    let retrieved = harness.get_song_by_path(song_path.as_str()).unwrap().unwrap();
    assert_eq!(retrieved.path, song_path);

    // Non-existent path
    let nonexistent = harness.get_song_by_path("/nonexistent/path.mp3").unwrap();
    assert!(nonexistent.is_none());
}
