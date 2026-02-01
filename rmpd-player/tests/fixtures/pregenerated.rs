/// Pre-generated test fixtures for CI
///
/// These fixtures are small audio files committed to the repository
/// to enable decoder tests without requiring FFmpeg installation.

use std::path::PathBuf;

/// Get the fixtures directory path
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples")
}

/// Get a specific fixture file
pub fn get_fixture(filename: &str) -> PathBuf {
    fixtures_dir().join(filename)
}

// Basic format fixtures (1 second, 1kHz sine wave, 44.1kHz stereo)
pub fn sine_1khz_flac() -> PathBuf {
    get_fixture("sine_1khz.flac")
}

pub fn sine_1khz_mp3() -> PathBuf {
    get_fixture("sine_1khz.mp3")
}

pub fn sine_1khz_ogg() -> PathBuf {
    get_fixture("sine_1khz.ogg")
}

pub fn sine_1khz_opus() -> PathBuf {
    get_fixture("sine_1khz.opus")
}

pub fn sine_1khz_m4a() -> PathBuf {
    get_fixture("sine_1khz.m4a")
}

pub fn sine_1khz_wav() -> PathBuf {
    get_fixture("sine_1khz.wav")
}

// Different frequency for seeking tests (440Hz)
pub fn sine_440hz_flac() -> PathBuf {
    get_fixture("sine_440hz.flac")
}

// Silence for artifact detection
pub fn silence_flac() -> PathBuf {
    get_fixture("silence.flac")
}

// High-resolution audio (24-bit, 96kHz)
pub fn highres_flac() -> PathBuf {
    get_fixture("highres.flac")
}

// Mono file
pub fn mono_flac() -> PathBuf {
    get_fixture("mono.flac")
}

/// Check if pre-generated fixtures exist
pub fn fixtures_available() -> bool {
    fixtures_dir().exists() && sine_1khz_flac().exists()
}
