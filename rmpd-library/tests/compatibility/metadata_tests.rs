/// Metadata extraction tests using pre-generated fixtures
///
/// These tests use pre-generated audio files (no FFmpeg required)
/// and validate that rmpd extracts metadata correctly from various formats.

use std::time::Duration;

use crate::common::rmpd_harness::RmpdTestHarness;
use crate::fixtures::pregenerated;

#[test]
fn test_flac_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    // Verify core metadata
    assert_eq!(song.title, Some("Test Song".to_string()));
    assert_eq!(song.artist, Some("Test Artist".to_string()));
    assert_eq!(song.album, Some("Test Album".to_string()));
    assert_eq!(song.genre, Some("Rock".to_string()));
    assert_eq!(song.date, Some("2024".to_string()));
    assert_eq!(song.track, Some(1));

    // Verify audio properties
    assert_eq!(song.sample_rate, Some(44100));
    assert_eq!(song.channels, Some(2));
    assert!(song.duration.is_some());

    // Duration should be ~1 second (±100ms tolerance)
    let duration_secs = song.duration.unwrap().as_secs_f64();
    assert!(duration_secs >= 0.9 && duration_secs <= 1.1,
        "Duration {} not within expected range", duration_secs);
}

#[test]
fn test_mp3_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_mp3();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.title, Some("Test Song MP3".to_string()));
    assert_eq!(song.artist, Some("Test Artist MP3".to_string()));
    assert_eq!(song.album, Some("Test Album MP3".to_string()));
    assert_eq!(song.genre, Some("Pop".to_string()));

    // MP3 should have bitrate
    assert!(song.bitrate.is_some());
}

#[test]
fn test_ogg_vorbis_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_ogg();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.title, Some("Test Song OGG".to_string()));
    assert_eq!(song.artist, Some("Test Artist OGG".to_string()));
    assert_eq!(song.album, Some("Test Album OGG".to_string()));
}

#[test]
fn test_opus_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_opus();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.title, Some("Test Song Opus".to_string()));
    assert_eq!(song.artist, Some("Test Artist Opus".to_string()));
    assert_eq!(song.album, Some("Test Album Opus".to_string()));

    // Opus has specific sample rate (48kHz)
    assert_eq!(song.sample_rate, Some(48000));
}

#[test]
fn test_m4a_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_m4a();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.title, Some("Test Song M4A".to_string()));
    assert_eq!(song.artist, Some("Test Artist M4A".to_string()));
    assert_eq!(song.album, Some("Test Album M4A".to_string()));
}

#[test]
fn test_wav_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_wav();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    // WAV may or may not preserve metadata depending on the tag format
    // Just verify we can read audio properties
    assert!(song.sample_rate.is_some());
    assert!(song.channels.is_some());
    assert!(song.duration.is_some());
}

#[test]
fn test_unicode_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::unicode_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    // Verify unicode characters are preserved
    assert_eq!(song.title, Some("テストソング".to_string()));
    assert_eq!(song.artist, Some("Тестовый исполнитель".to_string()));
    assert_eq!(song.album, Some("Τεστ Άλμπουμ".to_string()));
    assert_eq!(song.genre, Some("الموسيقى".to_string()));
}

#[test]
fn test_minimal_metadata() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::minimal_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    // Core tags should be present
    assert_eq!(song.title, Some("Minimal".to_string()));
    assert_eq!(song.artist, Some("Artist".to_string()));
    assert_eq!(song.album, Some("Album".to_string()));

    // Optional tags should be None
    assert_eq!(song.genre, None);
    assert_eq!(song.composer, None);
    assert_eq!(song.comment, None);
}

#[test]
fn test_extended_metadata() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::extended_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.title, Some("Extended Metadata".to_string()));
    assert_eq!(song.artist, Some("Extended Artist".to_string()));
    assert_eq!(song.album, Some("Extended Album".to_string()));
    assert_eq!(song.album_artist, Some("Various Artists".to_string()));
    assert_eq!(song.composer, Some("Test Composer".to_string()));
    assert_eq!(song.genre, Some("Jazz".to_string()));
    assert_eq!(song.track, Some(5));
    assert_eq!(song.disc, Some(2));
}

#[test]
fn test_metadata_extraction_performance() {
    let harness = RmpdTestHarness::new().unwrap();

    let formats = vec![
        pregenerated::basic_flac(),
        pregenerated::basic_mp3(),
        pregenerated::basic_ogg(),
    ];

    for path in formats {
        let start = std::time::Instant::now();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        let elapsed = start.elapsed();

        // Extraction should be fast (< 100ms)
        assert!(elapsed < Duration::from_millis(100),
            "Metadata extraction for {:?} took {:?} (> 100ms)",
            path.file_name().unwrap(), elapsed);

        // Verify at least basic metadata was extracted
        assert!(song.title.is_some());
        assert!(song.duration.is_some());
    }
}

#[test]
fn test_audio_properties_accuracy() {
    let harness = RmpdTestHarness::new().unwrap();

    // Test FLAC (lossless, 44.1kHz stereo)
    let flac_song = harness.extract_metadata(pregenerated::basic_flac().to_str().unwrap()).unwrap();
    assert_eq!(flac_song.sample_rate, Some(44100));
    assert_eq!(flac_song.channels, Some(2));
    assert!(flac_song.bits_per_sample.is_some());

    // Test MP3 (lossy)
    let mp3_song = harness.extract_metadata(pregenerated::basic_mp3().to_str().unwrap()).unwrap();
    assert_eq!(mp3_song.sample_rate, Some(44100));
    assert_eq!(mp3_song.channels, Some(2));
    assert!(mp3_song.bitrate.is_some());
}

#[test]
fn test_multiple_formats_consistency() {
    let harness = RmpdTestHarness::new().unwrap();

    let formats = vec![
        pregenerated::basic_flac(),
        pregenerated::basic_mp3(),
        pregenerated::basic_ogg(),
    ];

    for path in formats {
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

        // All should have basic metadata
        assert!(song.title.is_some(), "Missing title for {:?}", path.file_name());
        assert!(song.artist.is_some(), "Missing artist for {:?}", path.file_name());
        assert!(song.album.is_some(), "Missing album for {:?}", path.file_name());

        // All should have audio properties
        assert!(song.sample_rate.is_some());
        assert!(song.channels.is_some());
        assert!(song.duration.is_some());
    }
}
