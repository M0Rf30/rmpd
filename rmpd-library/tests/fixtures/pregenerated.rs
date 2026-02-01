/// Pre-generated test audio fixtures
///
/// These files are committed to the repository and do not require FFmpeg.
/// All files are ~1 second of audio with various metadata.

use std::path::PathBuf;

/// Get the path to the fixtures directory
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples")
}

/// Get path to a specific fixture file
pub fn get_fixture(filename: &str) -> PathBuf {
    fixtures_dir().join(filename)
}

/// Basic FLAC file with standard metadata
pub fn basic_flac() -> PathBuf {
    get_fixture("basic.flac")
}

/// Basic MP3 file with ID3v2 tags
pub fn basic_mp3() -> PathBuf {
    get_fixture("basic.mp3")
}

/// Basic OGG Vorbis file
pub fn basic_ogg() -> PathBuf {
    get_fixture("basic.ogg")
}

/// Basic Opus file (48kHz)
pub fn basic_opus() -> PathBuf {
    get_fixture("basic.opus")
}

/// Basic M4A/AAC file
pub fn basic_m4a() -> PathBuf {
    get_fixture("basic.m4a")
}

/// Basic WAV file
pub fn basic_wav() -> PathBuf {
    get_fixture("basic.wav")
}

/// FLAC with Unicode metadata (Japanese, Russian, Greek, Arabic)
pub fn unicode_flac() -> PathBuf {
    get_fixture("unicode.flac")
}

/// FLAC with minimal metadata (title, artist, album only)
pub fn minimal_flac() -> PathBuf {
    get_fixture("minimal.flac")
}

/// FLAC with extended metadata (composer, album artist, disc, track)
pub fn extended_flac() -> PathBuf {
    get_fixture("extended.flac")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixtures_exist() {
        // Verify all fixtures exist
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
        // Verify fixtures are reasonable size (not empty, not too large)
        let flac_size = std::fs::metadata(basic_flac()).unwrap().len();
        assert!(flac_size > 1000, "FLAC too small");
        assert!(flac_size < 100_000, "FLAC too large");

        let mp3_size = std::fs::metadata(basic_mp3()).unwrap().len();
        assert!(mp3_size > 1000, "MP3 too small");
        assert!(mp3_size < 50_000, "MP3 too large");
    }
}
