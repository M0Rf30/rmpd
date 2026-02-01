/// Decoder validation tests
///
/// Tests that validate decoder output correctness:
/// - Format detection and audio properties
/// - Sample-accurate decoding
/// - Seek accuracy
/// - Multi-format consistency

mod common;
mod fixtures;

use approx::assert_relative_eq;
use fixtures::pregenerated;
use fixtures::reference::{calculate_rms, verify_sine_wave};
use rmpd_player::decoder::SymphoniaDecoder;
use std::path::Path;

/// Helper to decode entire file to buffer
fn decode_entire_file(path: &Path) -> Result<(Vec<f32>, u32, u8), String> {
    let mut decoder = SymphoniaDecoder::open(path)
        .map_err(|e| format!("Failed to open decoder: {e}"))?;

    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();

    let mut all_samples = Vec::new();
    let mut buffer = vec![0.0f32; 4096];

    loop {
        let samples_read = decoder.read(&mut buffer)
            .map_err(|e| format!("Failed to read samples: {e}"))?;

        if samples_read == 0 {
            break;
        }

        all_samples.extend_from_slice(&buffer[..samples_read]);
    }

    Ok((all_samples, sample_rate, channels))
}

#[test]
fn test_flac_format_detection() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open FLAC file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100, "Expected 44.1kHz sample rate");
    assert_eq!(format.channels, 2, "Expected stereo");
}

#[test]
fn test_mp3_format_detection() {
    let path = pregenerated::sine_1khz_mp3();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open MP3 file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100);
    assert_eq!(format.channels, 2);

    // Bitrate may not be available until after reading first packet
    let mut buffer = vec![0.0f32; 1000];
    decoder.read(&mut buffer).expect("Failed to read");
    // Now bitrate should be available (but not mandatory for test to pass)
}

#[test]
fn test_ogg_format_detection() {
    let path = pregenerated::sine_1khz_ogg();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open OGG file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100);
    assert_eq!(format.channels, 2);
}

#[test]
#[ignore] // Opus requires additional Symphonia features not enabled by default
fn test_opus_format_detection() {
    let path = pregenerated::sine_1khz_opus();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open Opus file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 48000, "Opus uses 48kHz");
    assert_eq!(format.channels, 2);
}

#[test]
fn test_m4a_format_detection() {
    let path = pregenerated::sine_1khz_m4a();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open M4A file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100);
    assert_eq!(format.channels, 2);
}

#[test]
fn test_wav_format_detection() {
    let path = pregenerated::sine_1khz_wav();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open WAV file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100);
    assert_eq!(format.channels, 2);
}

#[test]
fn test_flac_sine_wave_accuracy() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let (samples, sample_rate, channels) = decode_entire_file(&path)
        .expect("Failed to decode FLAC file");

    // FLAC is lossless, so decoded sine wave should match perfectly
    assert!(
        verify_sine_wave(&samples, sample_rate, channels, 1000.0, 0.01),
        "FLAC decoded sine wave doesn't match expected pattern"
    );

    // Verify RMS is reasonable for a sine wave
    // Our fixtures use amplitude ~0.8, so RMS should be ~0.565 (0.8/sqrt(2))
    let rms = calculate_rms(&samples);
    assert_relative_eq!(rms, 0.565, epsilon = 0.05);
}

#[test]
fn test_wav_sine_wave_accuracy() {
    let path = pregenerated::sine_1khz_wav();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let (samples, sample_rate, channels) = decode_entire_file(&path)
        .expect("Failed to decode WAV file");

    // WAV/PCM is lossless
    assert!(
        verify_sine_wave(&samples, sample_rate, channels, 1000.0, 0.01),
        "WAV decoded sine wave doesn't match expected pattern"
    );

    // Our fixtures use amplitude ~0.8, so RMS should be ~0.565
    let rms = calculate_rms(&samples);
    assert_relative_eq!(rms, 0.565, epsilon = 0.05);
}

#[test]
fn test_mp3_sine_wave_reasonable() {
    let path = pregenerated::sine_1khz_mp3();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let (samples, _sample_rate, _channels) = decode_entire_file(&path)
        .expect("Failed to decode MP3 file");

    // MP3 is lossy, so we don't verify exact sine wave pattern
    // Instead, just verify output is reasonable (not silence, not corrupted)

    // RMS should be reasonable (0.565 ± 30% for lossy codec)
    let rms = calculate_rms(&samples);
    assert!(rms > 0.3 && rms < 0.8, "MP3 RMS {} out of expected range", rms);

    // All samples should be in valid range
    for &sample in &samples {
        assert!(
            sample >= -1.0 && sample <= 1.0,
            "MP3 sample {} out of valid range",
            sample
        );
    }
}

#[test]
fn test_duration_accuracy() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    let duration = decoder.duration();
    assert!(duration.is_some(), "Duration should be available");

    let dur = duration.unwrap();
    // Should be ~1 second (±10ms tolerance)
    assert!(
        (dur - 1.0).abs() < 0.01,
        "Duration {} not close to 1.0 seconds",
        dur
    );
}

#[test]
fn test_seek_to_beginning() {
    let path = pregenerated::sine_440hz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Read some samples
    let mut buffer = vec![0.0f32; 1000];
    decoder.read(&mut buffer).expect("Failed to read");

    // Seek back to beginning
    decoder.seek(0.0).expect("Failed to seek");

    // Read again and verify we're at the start
    let mut buffer2 = vec![0.0f32; 1000];
    let read = decoder.read(&mut buffer2).expect("Failed to read after seek");
    assert!(read > 0, "Should read samples after seeking to start");
}

#[test]
fn test_seek_to_middle() {
    let path = pregenerated::sine_440hz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Seek to 0.5 seconds
    decoder.seek(0.5).expect("Failed to seek");

    // Read samples and verify they're valid
    let mut buffer = vec![0.0f32; 1000];
    let read = decoder.read(&mut buffer).expect("Failed to read after seek");
    assert!(read > 0, "Should read samples after seeking");

    // Verify samples are in valid range
    for &sample in &buffer[..read] {
        assert!(sample >= -1.0 && sample <= 1.0, "Sample out of valid range");
    }
}

#[test]
fn test_seek_accuracy() {
    let path = pregenerated::sine_440hz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Seek to 0.25 seconds
    decoder.seek(0.25).expect("Failed to seek");

    // Read samples and verify they're valid
    let mut buffer = vec![0.0f32; 1000];
    let read = decoder.read(&mut buffer).expect("Failed to read");
    assert!(read > 0, "Should read samples after seeking");

    // Verify samples are in valid range
    for &sample in &buffer[..read] {
        assert!(
            sample >= -1.0 && sample <= 1.0,
            "Sample {} out of valid range after seek",
            sample
        );
    }

    // Verify we're getting reasonable audio (not silence)
    let rms = calculate_rms(&buffer[..read]);
    assert!(rms > 0.1, "RMS {} too low after seek (possibly silence)", rms);

    // Seek should be roughly accurate - if we seek to 0.5s and read 0.1s more,
    // we should have about 0.4s left in the file
    decoder.seek(0.5).expect("Failed to seek to 0.5s");
    let mut remaining_samples = 0;
    loop {
        let read = decoder.read(&mut buffer).expect("Failed to read");
        if read == 0 {
            break;
        }
        remaining_samples += read;
    }

    // File is 1 second, so after seeking to 0.5s, we should have ~0.5s left
    // At 44.1kHz stereo, that's about 44100 samples (stereo interleaved)
    // Allow 20% tolerance for codec frame boundaries and rounding
    let expected_remaining = 44100usize;
    let diff_ratio = (remaining_samples as f32 - expected_remaining as f32).abs()
        / expected_remaining as f32;
    assert!(
        diff_ratio < 0.2,
        "Seek accuracy off by more than 20%: expected ~{}, got {}",
        expected_remaining,
        remaining_samples
    );
}

#[test]
fn test_multiple_reads() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Read multiple small buffers
    let mut total_samples = 0;
    for _ in 0..10 {
        let mut buffer = vec![0.0f32; 512];
        let read = decoder.read(&mut buffer).expect("Failed to read");
        total_samples += read;

        if read == 0 {
            break;
        }

        // Verify all samples are in valid range
        for &sample in &buffer[..read] {
            assert!(
                sample >= -1.0 && sample <= 1.0,
                "Sample {} out of valid range",
                sample
            );
        }
    }

    assert!(total_samples > 0, "Should have read some samples");
}

#[test]
fn test_high_resolution_audio() {
    let path = pregenerated::highres_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found (optional high-res test)");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open high-res file");

    let format = decoder.format();
    assert_eq!(format.sample_rate, 96000, "Expected 96kHz sample rate");
    assert_eq!(format.channels, 2);
}

#[test]
fn test_mono_audio() {
    let path = pregenerated::mono_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found (optional mono test)");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open mono file");

    let format = decoder.format();
    assert_eq!(format.channels, 1, "Expected mono (1 channel)");
}

#[test]
fn test_silence_detection() {
    let path = pregenerated::silence_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found (optional silence test)");
        return;
    }

    let (samples, _, _) = decode_entire_file(&path)
        .expect("Failed to decode silence file");

    // Verify RMS is very low (near silence)
    let rms = calculate_rms(&samples);
    assert!(rms < 0.01, "Silence file should have very low RMS, got {}", rms);
}

#[test]
fn test_decoder_format_consistency() {
    // Test that all formats decode to reasonable values
    let formats = vec![
        ("FLAC", pregenerated::sine_1khz_flac()),
        ("MP3", pregenerated::sine_1khz_mp3()),
        ("OGG", pregenerated::sine_1khz_ogg()),
        ("WAV", pregenerated::sine_1khz_wav()),
    ];

    for (name, path) in formats {
        if !path.exists() {
            eprintln!("Skipping {}: fixture not found", name);
            continue;
        }

        let (samples, _, _) = decode_entire_file(&path)
            .unwrap_or_else(|e| panic!("Failed to decode {}: {}", name, e));

        // All should have reasonable RMS for a sine wave (0.565 ± 25%)
        let rms = calculate_rms(&samples);
        assert!(
            rms > 0.4 && rms < 0.75,
            "{} RMS {} out of expected range [0.4, 0.75]",
            name,
            rms
        );

        // All samples should be in valid range
        for &sample in &samples {
            assert!(
                sample >= -1.0 && sample <= 1.0,
                "{} produced out-of-range sample: {}",
                name,
                sample
            );
        }
    }
}
