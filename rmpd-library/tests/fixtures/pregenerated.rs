/// Pre-generated test audio fixtures
///
/// These files are committed to the repository and do not require FFmpeg.
/// All files are ~1 second of audio with various metadata.
use std::path::PathBuf;

use rmpd_core::test_utils;

#[allow(dead_code)]
pub fn fixtures_dir() -> PathBuf {
    test_utils::fixtures_dir(env!("CARGO_MANIFEST_DIR"))
}

pub fn get_fixture(filename: &str) -> PathBuf {
    test_utils::get_fixture(env!("CARGO_MANIFEST_DIR"), filename)
}

pub fn basic_flac() -> PathBuf {
    get_fixture("basic.flac")
}

pub fn basic_mp3() -> PathBuf {
    get_fixture("basic.mp3")
}

pub fn basic_ogg() -> PathBuf {
    get_fixture("basic.ogg")
}

pub fn basic_opus() -> PathBuf {
    get_fixture("basic.opus")
}

pub fn basic_m4a() -> PathBuf {
    get_fixture("basic.m4a")
}

pub fn basic_wav() -> PathBuf {
    get_fixture("basic.wav")
}

pub fn unicode_flac() -> PathBuf {
    get_fixture("unicode.flac")
}

pub fn minimal_flac() -> PathBuf {
    get_fixture("minimal.flac")
}

pub fn extended_flac() -> PathBuf {
    get_fixture("extended.flac")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixtures_exist() {
        assert!(basic_flac().exists(), "basic.flac not found");
        assert!(basic_mp3().exists(), "basic.mp3 not found");
        assert!(basic_ogg().exists(), "basic.ogg not found");
        assert!(basic_opus().exists(), "basic.opus not found");
        assert!(basic_m4a().exists(), "basic.m4a not found");
        assert!(basic_wav().exists(), "basic.wav not found");
        assert!(unicode_flac().exists(), "unicode.flac not found");
        assert!(minimal_flac().exists(), "minimal.flac not found");
        assert!(extended_flac().exists(), "extended.flac not found");
    }

    #[test]
    fn test_fixture_sizes() {
        let flac_size = std::fs::metadata(basic_flac()).unwrap().len();
        assert!(flac_size > 1000, "FLAC too small");
        assert!(flac_size < 100_000, "FLAC too large");

        let mp3_size = std::fs::metadata(basic_mp3()).unwrap().len();
        assert!(mp3_size > 1000, "MP3 too small");
        assert!(mp3_size < 50_000, "MP3 too large");
    }
}
