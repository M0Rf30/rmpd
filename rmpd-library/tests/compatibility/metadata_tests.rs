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
    assert_eq!(song.tag("title"), Some("Test Song"));
    assert_eq!(song.tag("artist"), Some("Test Artist"));
    assert_eq!(song.tag("album"), Some("Test Album"));
    assert_eq!(song.tag("genre"), Some("Rock"));
    assert_eq!(song.tag("date"), Some("2024"));
    assert_eq!(song.tag("track"), Some("1"));

    // Verify audio properties
    assert_eq!(song.sample_rate, Some(44100));
    assert_eq!(song.channels, Some(2));
    assert!(song.duration.is_some());

    // Duration should be ~1 second (±100ms tolerance)
    let duration_secs = song.duration.unwrap().as_secs_f64();
    assert!(
        (0.9..=1.1).contains(&duration_secs),
        "Duration {} not within expected range",
        duration_secs
    );
}

#[test]
fn test_mp3_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_mp3();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.tag("title"), Some("Test Song MP3"));
    assert_eq!(song.tag("artist"), Some("Test Artist MP3"));
    assert_eq!(song.tag("album"), Some("Test Album MP3"));
    assert_eq!(song.tag("genre"), Some("Pop"));

    // MP3 should have bitrate
    assert!(song.bitrate.is_some());
}

#[test]
fn test_ogg_vorbis_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_ogg();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.tag("title"), Some("Test Song OGG"));
    assert_eq!(song.tag("artist"), Some("Test Artist OGG"));
    assert_eq!(song.tag("album"), Some("Test Album OGG"));
}

#[test]
fn test_opus_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_opus();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.tag("title"), Some("Test Song Opus"));
    assert_eq!(song.tag("artist"), Some("Test Artist Opus"));
    assert_eq!(song.tag("album"), Some("Test Album Opus"));

    // Opus has specific sample rate (48kHz)
    assert_eq!(song.sample_rate, Some(48000));
}

#[test]
fn test_m4a_metadata_extraction() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::basic_m4a();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.tag("title"), Some("Test Song M4A"));
    assert_eq!(song.tag("artist"), Some("Test Artist M4A"));
    assert_eq!(song.tag("album"), Some("Test Album M4A"));
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
    assert_eq!(song.tag("title"), Some("テストソング"));
    assert_eq!(song.tag("artist"), Some("Тестовый исполнитель"));
    assert_eq!(song.tag("album"), Some("Τεστ Άλμπουμ"));
    assert_eq!(song.tag("genre"), Some("الموسيقى"));
}

#[test]
fn test_minimal_metadata() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::minimal_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    // Core tags should be present
    assert_eq!(song.tag("title"), Some("Minimal"));
    assert_eq!(song.tag("artist"), Some("Artist"));
    assert_eq!(song.tag("album"), Some("Album"));

    // Optional tags should be None
    assert!(song.tag("genre").is_none());
    assert!(song.tag("composer").is_none());
    assert!(song.tag("comment").is_none());
}

#[test]
fn test_extended_metadata() {
    let harness = RmpdTestHarness::new().unwrap();
    let path = pregenerated::extended_flac();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();

    assert_eq!(song.tag("title"), Some("Extended Metadata"));
    assert_eq!(song.tag("artist"), Some("Extended Artist"));
    assert_eq!(song.tag("album"), Some("Extended Album"));
    assert_eq!(song.tag("albumartist"), Some("Various Artists"));
    assert_eq!(song.tag("composer"), Some("Test Composer"));
    assert_eq!(song.tag("genre"), Some("Jazz"));
    assert_eq!(song.tag("track"), Some("5"));
    assert_eq!(song.tag("disc"), Some("2"));
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
        assert!(
            elapsed < Duration::from_millis(100),
            "Metadata extraction for {:?} took {:?} (> 100ms)",
            path.file_name().unwrap(),
            elapsed
        );

        // Verify at least basic metadata was extracted
        assert!(song.tag("title").is_some());
        assert!(song.duration.is_some());
    }
}

#[test]
fn test_audio_properties_accuracy() {
    let harness = RmpdTestHarness::new().unwrap();

    // Test FLAC (lossless, 44.1kHz stereo)
    let flac_song = harness
        .extract_metadata(pregenerated::basic_flac().to_str().unwrap())
        .unwrap();
    assert_eq!(flac_song.sample_rate, Some(44100));
    assert_eq!(flac_song.channels, Some(2));
    assert!(flac_song.bits_per_sample.is_some());

    // Test MP3 (lossy)
    let mp3_song = harness
        .extract_metadata(pregenerated::basic_mp3().to_str().unwrap())
        .unwrap();
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
        assert!(
            song.tag("title").is_some(),
            "Missing title for {:?}",
            path.file_name()
        );
        assert!(
            song.tag("artist").is_some(),
            "Missing artist for {:?}",
            path.file_name()
        );
        assert!(
            song.tag("album").is_some(),
            "Missing album for {:?}",
            path.file_name()
        );

        // All should have audio properties
        assert!(song.sample_rate.is_some());
        assert!(song.channels.is_some());
        assert!(song.duration.is_some());
    }
}
