/// Demo test showing the database compatibility test infrastructure
///
/// This demonstrates the pattern for comparing rmpd behavior with MPD.
/// Full implementation would include MPD harness and real audio file fixtures.

mod common;

use common::comparison::{assert_songs_match, ComparisonConfig};
use common::rmpd_harness::RmpdTestHarness;
use rmpd_core::song::Song;
use std::time::Duration;

fn create_demo_song(id: u64, artist: &str) -> Song {
    Song {
        id,
        path: format!("/music/{artist}/song.mp3").into(),
        duration: Some(Duration::from_secs(180)),
        title: Some(format!("Song by {artist}")),
        artist: Some(artist.to_string()),
        album: Some("Demo Album".to_string()),
        album_artist: None,
        track: Some(1),
        disc: None,
        date: Some("2024".to_string()),
        genre: Some("Rock".to_string()),
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
    }
}

#[test]
fn test_rmpd_harness_basic_operations() {
    let harness = RmpdTestHarness::new().unwrap();

    // Add some songs
    let song1 = create_demo_song(0, "Artist A");
    let song2 = create_demo_song(0, "Artist B");
    let song3 = create_demo_song(0, "Artist A");

    harness.add_song(&song1).unwrap();
    harness.add_song(&song2).unwrap();
    harness.add_song(&song3).unwrap();

    // Test counts
    assert_eq!(harness.count_songs().unwrap(), 3);
    assert_eq!(harness.count_artists().unwrap(), 2);

    // Test list operations
    let artists = harness.list_artists().unwrap();
    assert_eq!(artists.len(), 2);
    assert!(artists.contains(&"Artist A".to_string()));
    assert!(artists.contains(&"Artist B".to_string()));

    // Test find operations
    let found = harness.find_by_artist("Artist A").unwrap();
    assert_eq!(found.len(), 2);

    // Test search (FTS)
    let search_results = harness.search("Artist A").unwrap();
    assert_eq!(search_results.len(), 2);
}

#[test]
fn test_comparison_utilities() {
    // This demonstrates the comparison infrastructure
    // In real tests, song2 would come from MPD harness
    let song1 = create_demo_song(1, "Test Artist");
    let song2 = song1.clone();

    let config = ComparisonConfig::default();

    // This would be used in actual comparison tests
    assert_songs_match(&song1, &song2, &config);
}

#[test]
fn test_comparison_with_tolerance() {
    let mut song1 = create_demo_song(1, "Test Artist");
    let mut song2 = create_demo_song(1, "Test Artist");

    // Simulate slight differences that are within tolerance
    song1.duration = Some(Duration::from_millis(180_500)); // 180.5s
    song2.duration = Some(Duration::from_millis(181_200)); // 181.2s (0.7s diff, within ±1s)

    song1.bitrate = Some(320);
    song2.bitrate = Some(315); // 1.6% difference, within ±10%

    let config = ComparisonConfig::default();
    assert_songs_match(&song1, &song2, &config);
}

#[test]
fn test_database_query_operations() {
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from multiple artists and albums
    let mut song1 = create_demo_song(0, "Rock Band");
    song1.album = Some("First Album".to_string());
    harness.add_song(&song1).unwrap();

    let mut song2 = create_demo_song(0, "Rock Band");
    song2.album = Some("Second Album".to_string());
    harness.add_song(&song2).unwrap();

    let mut song3 = create_demo_song(0, "Jazz Artist");
    song3.genre = Some("Jazz".to_string());
    harness.add_song(&song3).unwrap();

    // Test list operations
    let albums = harness.list_albums().unwrap();
    assert_eq!(albums.len(), 3); // Demo Album (from Jazz Artist), First Album, Second Album

    // Test find operations
    let rock_songs = harness.find_by_artist("Rock Band").unwrap();
    assert_eq!(rock_songs.len(), 2);

    // Test search with partial match
    let search_results = harness.search("Rock").unwrap();
    assert!(search_results.len() >= 2); // Should find "Rock Band" songs
}

// This demonstrates the pattern for full comparison tests
// In actual implementation, this would use MPD harness
#[test]
#[ignore] // Requires MPD installation
fn test_metadata_extraction_comparison() {
    // This is the pattern for actual comparison tests:
    //
    // 1. Generate test file with FFmpeg
    // let fixture = generate_flac_test_file();
    //
    // 2. Extract with rmpd
    // let rmpd_harness = RmpdTestHarness::new().unwrap();
    // let rmpd_song = rmpd_harness.extract_metadata(&fixture).unwrap();
    //
    // 3. Extract with MPD
    // let mpd_harness = MpdTestHarness::spawn().unwrap();
    // mpd_harness.copy_file(&fixture).await;
    // let mpd_song = mpd_harness.get_song_metadata(&fixture).await;
    //
    // 4. Compare
    // assert_songs_match(&rmpd_song, &mpd_song, &ComparisonConfig::default());
}
