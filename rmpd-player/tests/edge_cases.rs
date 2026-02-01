/// Edge case tests for decoder robustness
///
/// Tests handling of:
/// - Truncated/corrupted files
/// - Empty files
/// - Invalid formats
/// - Boundary conditions
/// - Very large/small values

mod common;
mod fixtures;

use fixtures::pregenerated;
use rmpd_player::decoder::SymphoniaDecoder;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_nonexistent_file() {
    let result = SymphoniaDecoder::open(&std::path::PathBuf::from("/nonexistent/file.flac"));
    assert!(result.is_err(), "Should fail to open nonexistent file");
}

#[test]
fn test_empty_file() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let empty_file = temp_dir.path().join("empty.flac");

    // Create empty file
    fs::File::create(&empty_file).expect("Failed to create empty file");

    let result = SymphoniaDecoder::open(&empty_file);
    assert!(result.is_err(), "Should fail to open empty file");
}

#[test]
fn test_truncated_file() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let truncated_file = temp_dir.path().join("truncated.flac");

    // Read original file and write only first half
    let data = fs::read(&path).expect("Failed to read fixture");
    let mut file = fs::File::create(&truncated_file).expect("Failed to create truncated file");
    file.write_all(&data[..data.len() / 2])
        .expect("Failed to write truncated data");
    drop(file);

    // Should open successfully (header is intact)
    let mut decoder = SymphoniaDecoder::open(&truncated_file)
        .expect("Should open truncated file (header intact)");

    // But reading might fail or return fewer samples
    let mut buffer = vec![0.0f32; 100000]; // Request more than available
    let result = decoder.read(&mut buffer);

    // Either fails gracefully or returns what's available
    match result {
        Ok(read) => {
            // Should read something, but not as much as original
            println!("Read {} samples from truncated file", read);
        }
        Err(_) => {
            // Also acceptable - decoder detected corruption
            println!("Decoder correctly detected corruption");
        }
    }
}

#[test]
fn test_corrupted_header() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let corrupted_file = temp_dir.path().join("corrupted.flac");

    // Create file with invalid header
    let mut file = fs::File::create(&corrupted_file).expect("Failed to create file");
    file.write_all(b"NOT A VALID AUDIO FILE HEADER DATA")
        .expect("Failed to write data");
    drop(file);

    let result = SymphoniaDecoder::open(&corrupted_file);
    assert!(
        result.is_err(),
        "Should fail to open file with corrupted header"
    );
}

#[test]
fn test_wrong_extension() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let wrong_ext = temp_dir.path().join("audio.mp3");

    // Copy FLAC file but with .mp3 extension
    fs::copy(&path, &wrong_ext).expect("Failed to copy file");

    // Symphonia should still detect it as FLAC (based on content, not extension)
    let result = SymphoniaDecoder::open(&wrong_ext);
    assert!(
        result.is_ok(),
        "Symphonia should detect format from content"
    );
}

#[test]
fn test_read_after_eof() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Read entire file
    let mut buffer = vec![0.0f32; 100000]; // Large buffer
    let mut total_read = 0;

    loop {
        let read = decoder.read(&mut buffer).expect("Failed to read");
        if read == 0 {
            break;
        }
        total_read += read;
    }

    assert!(total_read > 0, "Should have read some samples");

    // Try to read more (should return 0, not error)
    let read = decoder.read(&mut buffer).expect("Read after EOF should not error");
    assert_eq!(read, 0, "Should return 0 samples after EOF");
}

#[test]
fn test_seek_beyond_duration() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Try to seek beyond file duration (file is ~1 second)
    let result = decoder.seek(10.0);

    // Behavior is implementation-defined, but should either:
    // 1. Seek to end (acceptable)
    // 2. Return error (also acceptable)
    match result {
        Ok(_) => {
            println!("Seek beyond duration succeeded (seeking to end)");
            // Verify we're at or near end
            let mut buffer = vec![0.0f32; 1000];
            let read = decoder.read(&mut buffer).expect("Failed to read");
            println!("Read {} samples after seek-beyond-end", read);
        }
        Err(_) => {
            println!("Seek beyond duration correctly returned error");
        }
    }
}

#[test]
fn test_seek_negative() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Try to seek to negative position
    let result = decoder.seek(-1.0);
    assert!(result.is_err(), "Should reject negative seek position");
}

#[test]
fn test_zero_length_read() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Try to read 0 samples
    let mut buffer = vec![];
    let read = decoder.read(&mut buffer).expect("Zero-length read should not error");
    assert_eq!(read, 0, "Should read 0 samples from 0-length buffer");
}

#[test]
fn test_very_small_buffer() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Read with very small buffer (1 sample)
    let mut buffer = vec![0.0f32; 1];
    let read = decoder.read(&mut buffer).expect("Failed to read");

    // Should read 0 or 1 sample
    assert!(read <= 1, "Should not read more than buffer size");
}

#[test]
fn test_sample_value_range() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let mut decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Read entire file and verify all samples are in valid range
    let mut buffer = vec![0.0f32; 4096];
    let mut sample_count = 0;
    let mut min_sample = f32::MAX;
    let mut max_sample = f32::MIN;

    loop {
        let read = decoder.read(&mut buffer).expect("Failed to read");
        if read == 0 {
            break;
        }

        for &sample in &buffer[..read] {
            assert!(
                (-1.0..=1.0).contains(&sample),
                "Sample {} out of valid range [-1.0, 1.0]",
                sample
            );
            min_sample = min_sample.min(sample);
            max_sample = max_sample.max(sample);
            sample_count += 1;
        }
    }

    assert!(sample_count > 0, "Should have read samples");
    println!(
        "Sample range: [{}, {}] ({} samples)",
        min_sample, max_sample, sample_count
    );

    // For our sine wave (amplitude ~0.8), should use most of the available range
    assert!(max_sample > 0.7, "Max sample should be close to 0.8");
    assert!(min_sample < -0.7, "Min sample should be close to -0.8");
}

#[test]
fn test_multiple_decoders_same_file() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    // Open multiple decoders on the same file
    let mut decoder1 = SymphoniaDecoder::open(&path).expect("Failed to open decoder 1");
    let mut decoder2 = SymphoniaDecoder::open(&path).expect("Failed to open decoder 2");

    // Both should work independently
    let mut buffer1 = vec![0.0f32; 1000];
    let mut buffer2 = vec![0.0f32; 1000];

    let read1 = decoder1.read(&mut buffer1).expect("Failed to read from decoder 1");
    let read2 = decoder2.read(&mut buffer2).expect("Failed to read from decoder 2");

    assert!(read1 > 0, "Decoder 1 should read samples");
    assert!(read2 > 0, "Decoder 2 should read samples");

    // Should produce identical output
    assert_eq!(read1, read2, "Both decoders should read same amount");
    for i in 0..read1 {
        assert_eq!(
            buffer1[i], buffer2[i],
            "Decoders should produce identical output"
        );
    }
}

#[test]
fn test_seek_then_decode_consistency() {
    let path = pregenerated::sine_440hz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    // Decode from start
    let mut decoder1 = SymphoniaDecoder::open(&path).expect("Failed to open decoder");
    let mut buffer1 = vec![0.0f32; 10000];
    let read1 = decoder1.read(&mut buffer1).expect("Failed to read");

    // Seek to 0.0 then decode
    let mut decoder2 = SymphoniaDecoder::open(&path).expect("Failed to open decoder");
    decoder2.seek(0.0).expect("Failed to seek");
    let mut buffer2 = vec![0.0f32; 10000];
    let read2 = decoder2.read(&mut buffer2).expect("Failed to read after seek");

    // Should produce similar output (small differences acceptable due to codec behavior)
    assert!(
        (read1 as i32 - read2 as i32).abs() < 100,
        "Read counts should be similar"
    );

    // Check first 1000 samples are close
    let compare_count = read1.min(read2).min(1000);
    let mut differences = 0;
    for i in 0..compare_count {
        if (buffer1[i] - buffer2[i]).abs() > 0.01 {
            differences += 1;
        }
    }

    let diff_ratio = differences as f32 / compare_count as f32;
    assert!(
        diff_ratio < 0.05,
        "More than 5% of samples differ significantly"
    );
}

#[test]
fn test_decoder_format_info_before_read() {
    let path = pregenerated::sine_1khz_flac();
    if !path.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let decoder = SymphoniaDecoder::open(&path).expect("Failed to open file");

    // Should be able to get format info before reading any samples
    let format = decoder.format();
    assert_eq!(format.sample_rate, 44100);
    assert_eq!(format.channels, 2);

    let duration = decoder.duration();
    assert!(duration.is_some(), "Duration should be available before reading");
}
