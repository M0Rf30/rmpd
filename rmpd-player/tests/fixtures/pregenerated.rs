/// Pre-generated test fixtures for CI
///
/// These fixtures are small audio files committed to the repository
/// to enable decoder tests without requiring FFmpeg installation.
use std::path::PathBuf;

use rmpd_core::test_utils;

#[allow(dead_code)]
pub fn fixtures_dir() -> PathBuf {
    test_utils::fixtures_dir(env!("CARGO_MANIFEST_DIR"))
}

#[allow(dead_code)]
pub fn get_fixture(filename: &str) -> PathBuf {
    test_utils::get_fixture(env!("CARGO_MANIFEST_DIR"), filename)
}

#[allow(dead_code)]
pub fn sine_1khz_flac() -> PathBuf {
    get_fixture("sine_1khz.flac")
}

#[allow(dead_code)]
pub fn sine_1khz_mp3() -> PathBuf {
    get_fixture("sine_1khz.mp3")
}

#[allow(dead_code)]
pub fn sine_1khz_ogg() -> PathBuf {
    get_fixture("sine_1khz.ogg")
}

#[allow(dead_code)]
pub fn sine_1khz_opus() -> PathBuf {
    get_fixture("sine_1khz.opus")
}

#[allow(dead_code)]
pub fn sine_1khz_m4a() -> PathBuf {
    get_fixture("sine_1khz.m4a")
}

#[allow(dead_code)]
pub fn sine_1khz_wav() -> PathBuf {
    get_fixture("sine_1khz.wav")
}

#[allow(dead_code)]
pub fn sine_440hz_flac() -> PathBuf {
    get_fixture("sine_440hz.flac")
}

#[allow(dead_code)]
pub fn silence_flac() -> PathBuf {
    get_fixture("silence.flac")
}

#[allow(dead_code)]
pub fn highres_flac() -> PathBuf {
    get_fixture("highres.flac")
}

#[allow(dead_code)]
pub fn mono_flac() -> PathBuf {
    get_fixture("mono.flac")
}

#[allow(dead_code)]
pub fn fixtures_available() -> bool {
    fixtures_dir().exists() && sine_1khz_flac().exists()
}
