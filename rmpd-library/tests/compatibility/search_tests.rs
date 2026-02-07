/// Search operation tests validating FTS (Full-Text Search) functionality
///
/// These tests validate that rmpd's FTS5-based search produces
/// expected results for various query patterns.
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
fn test_full_text_search_basic() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs with searchable content
    let songs_data = vec![
        ("Bohemian Rhapsody", "Queen", "A Night at the Opera"),
        ("Stairway to Heaven", "Led Zeppelin", "Led Zeppelin IV"),
        ("Hotel California", "Eagles", "Hotel California"),
    ];

    for (title, artist, album) in songs_data {
        let metadata = TestMetadata {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Search for "Queen"
    let results = harness.search("Queen").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].artist, Some("Queen".to_string()));

    // Search for "California"
    let results = harness.search("California").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, Some("Hotel California".to_string()));

    // Search for "Led"
    let results = harness.search("Led").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].artist, Some("Led Zeppelin".to_string()));
}

#[test]
fn test_case_insensitive_search() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Thunder Road".to_string(),
        artist: "Bruce Springsteen".to_string(),
        album: "Born to Run".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // All variations should find the song
    let test_queries = vec!["thunder", "THUNDER", "Thunder", "ThUnDeR"];

    for query in test_queries {
        let results = harness.search(query).unwrap();
        assert_eq!(results.len(), 1, "Query '{}' should find 1 result", query);
        assert_eq!(results[0].title, Some("Thunder Road".to_string()));
    }
}

#[test]
fn test_partial_word_search() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Superstition".to_string(),
        artist: "Stevie Wonder".to_string(),
        album: "Talking Book".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // Prefix matching works in FTS5
    let results = harness.search("Super*").unwrap();
    assert!(
        !results.is_empty(),
        "Should find songs starting with 'Super'"
    );

    let results = harness.search("Stev*").unwrap();
    assert!(
        !results.is_empty(),
        "Should find songs with artist starting with 'Stev'"
    );
}

#[test]
fn test_multi_field_search() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add multiple songs
    let songs = vec![
        ("Dark Side", "Pink Floyd", "Dark Side of the Moon"),
        ("Wish You Were Here", "Pink Floyd", "Wish You Were Here"),
        ("The Dark Knight", "Various", "Movie Soundtrack"),
    ];

    for (title, artist, album) in songs {
        let metadata = TestMetadata {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Search for "Floyd" should find 2 songs
    let results = harness.search("Floyd").unwrap();
    assert_eq!(results.len(), 2);
    for song in &results {
        assert_eq!(song.artist, Some("Pink Floyd".to_string()));
    }

    // Search for "Dark" should find 2 songs (one with "Dark" in title, one in album)
    let results = harness.search("Dark").unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_unicode_search() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add song with unicode metadata
    let metadata = TestMetadata {
        title: "さくら".to_string(),          // "Sakura" in Japanese
        artist: "いきものがかり".to_string(), // "Ikimonogakari" in Japanese
        album: "桜咲く".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // Search with unicode characters
    let results = harness.search("さくら").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, Some("さくら".to_string()));

    // Search artist name
    let results = harness.search("いきものがかり").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].artist, Some("いきものがかり".to_string()));
}

#[test]
fn test_search_by_genre() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs with different genres
    let songs = vec![
        ("Song 1", "Artist 1", "Album 1", "Rock"),
        ("Song 2", "Artist 2", "Album 2", "Jazz"),
        ("Song 3", "Artist 3", "Album 3", "Rock"),
    ];

    for (title, artist, album, genre) in songs {
        let metadata = TestMetadata {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            genre: Some(genre.to_string()),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Search for "Rock" genre
    let results = harness.search("Rock").unwrap();
    assert_eq!(results.len(), 2);
    for song in &results {
        assert_eq!(song.genre, Some("Rock".to_string()));
    }

    // Search for "Jazz" genre
    let results = harness.search("Jazz").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].genre, Some("Jazz".to_string()));
}

#[test]
fn test_search_by_composer() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Symphony No. 5".to_string(),
        artist: "Berlin Philharmonic".to_string(),
        album: "Beethoven Symphonies".to_string(),
        composer: Some("Ludwig van Beethoven".to_string()),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // Search for composer
    let results = harness.search("Beethoven").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].composer,
        Some("Ludwig van Beethoven".to_string())
    );
}

#[test]
fn test_empty_search_results() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Test Song".to_string(),
        artist: "Test Artist".to_string(),
        album: "Test Album".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // Search for something that doesn't exist
    let results = harness.search("NonexistentQuery").unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn test_search_with_special_characters() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Don't Stop Believin'".to_string(),
        artist: "Journey".to_string(),
        album: "Escape".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    // Search should handle apostrophes
    let results = harness.search("Don't").unwrap();
    assert!(!results.is_empty());

    let results = harness.search("Believin").unwrap();
    assert!(!results.is_empty());
}

#[test]
fn test_search_performance() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add many songs
    for i in 1..=100 {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: format!("Artist {}", i % 10), // 10 different artists
            album: format!("Album {}", i % 5),    // 5 different albums
            genre: Some(if i % 2 == 0 {
                "Rock".to_string()
            } else {
                "Pop".to_string()
            }),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Search should be fast even with 100 songs
    let start = std::time::Instant::now();
    let results = harness.search("Rock").unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "Search took {:?}, expected < 50ms",
        elapsed
    );

    assert_eq!(results.len(), 50); // Half the songs are Rock
}
