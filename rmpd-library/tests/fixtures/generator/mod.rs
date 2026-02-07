/// FFmpeg-based test fixture generation
///
/// This module generates minimal audio files for testing metadata extraction.
/// Files are cached in target/test-fixtures/ to avoid regenerating on each test run.
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Audio format for test fixture generation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Flac,
    Mp3,
    Ogg,
    Wav,
}

impl AudioFormat {
    pub fn extension(&self) -> &str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Wav => "wav",
        }
    }

    pub fn codec(&self) -> &str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "libmp3lame",
            AudioFormat::Ogg => "libvorbis",
            AudioFormat::Wav => "pcm_s16le",
        }
    }
}

/// Metadata tags to embed in test files
#[derive(Debug, Clone)]
pub struct TestMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
}

impl Default for TestMetadata {
    fn default() -> Self {
        Self {
            title: "Test Song".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            album_artist: None,
            genre: Some("Rock".to_string()),
            date: Some("2024".to_string()),
            track: Some(1),
            disc: None,
            composer: None,
            comment: None,
        }
    }
}

/// Test fixture generator
pub struct FixtureGenerator {
    cache_dir: PathBuf,
}

impl FixtureGenerator {
    /// Create a new generator with caching in target/test-fixtures
    pub fn new() -> Result<Self, String> {
        let cache_dir = PathBuf::from("target/test-fixtures");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        Ok(Self { cache_dir })
    }

    /// Check if FFmpeg is available
    pub fn is_ffmpeg_available() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    /// Sanitize a string for use in filenames
    fn sanitize_for_filename(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                ' ' => '_',
                c => c,
            })
            .collect()
    }

    /// Generate a test audio file with metadata
    pub fn generate(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<PathBuf, String> {
        // Create cache key from format and metadata (sanitize for filesystem)
        let mut cache_key = format!(
            "{}_{}_{}_{}_{}",
            format.extension(),
            Self::sanitize_for_filename(&metadata.title),
            Self::sanitize_for_filename(&metadata.artist),
            Self::sanitize_for_filename(&metadata.album),
            Self::sanitize_for_filename(metadata.genre.as_deref().unwrap_or("noGenre"))
        );

        // Truncate cache key if too long (max filename length is typically 255)
        // Reserve space for extension (.flac = 5 chars)
        if cache_key.len() > 200 {
            cache_key.truncate(200);
        }

        let cached_path = self
            .cache_dir
            .join(&cache_key)
            .with_extension(format.extension());

        // Return cached file if it exists
        if cached_path.exists() {
            return Ok(cached_path);
        }

        // Generate new file
        self.generate_uncached(format, metadata, &cached_path)?;
        Ok(cached_path)
    }

    fn generate_uncached(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
        output_path: &Path,
    ) -> Result<(), String> {
        if !Self::is_ffmpeg_available() {
            return Err(
                "FFmpeg not available - install with: sudo apt-get install ffmpeg".to_string(),
            );
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-f").arg("lavfi")
            .arg("-i").arg("sine=frequency=440:duration=1") // 1 second 440Hz sine wave
            .arg("-ac").arg("2") // Stereo
            .arg("-y"); // Overwrite output

        // Set sample rate
        cmd.arg("-ar").arg("44100");

        // Add codec
        cmd.arg("-codec:a").arg(format.codec());

        // Add quality settings
        match format {
            AudioFormat::Mp3 => {
                cmd.arg("-q:a").arg("2"); // VBR quality 2
            }
            AudioFormat::Ogg => {
                cmd.arg("-b:a").arg("128k");
            }
            _ => {}
        }

        // Add metadata
        cmd.arg("-metadata")
            .arg(format!("title={}", metadata.title));
        cmd.arg("-metadata")
            .arg(format!("artist={}", metadata.artist));
        cmd.arg("-metadata")
            .arg(format!("album={}", metadata.album));

        if let Some(ref album_artist) = metadata.album_artist {
            cmd.arg("-metadata")
                .arg(format!("album_artist={}", album_artist));
        }

        if let Some(ref genre) = metadata.genre {
            cmd.arg("-metadata").arg(format!("genre={}", genre));
        }

        if let Some(ref date) = metadata.date {
            cmd.arg("-metadata").arg(format!("date={}", date));
        }

        if let Some(track) = metadata.track {
            cmd.arg("-metadata").arg(format!("track={}", track));
        }

        if let Some(disc) = metadata.disc {
            cmd.arg("-metadata").arg(format!("disc={}", disc));
        }

        if let Some(ref composer) = metadata.composer {
            cmd.arg("-metadata").arg(format!("composer={}", composer));
        }

        if let Some(ref comment) = metadata.comment {
            cmd.arg("-metadata").arg(format!("comment={}", comment));
        }

        cmd.arg(output_path);

        // Execute FFmpeg
        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run FFmpeg: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFmpeg failed: {}", stderr));
        }

        Ok(())
    }

    /// Generate a test file in a temporary directory (not cached)
    pub fn generate_temp(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<(TempDir, PathBuf), String> {
        let temp_dir = TempDir::new().map_err(|e| format!("Failed to create temp dir: {}", e))?;

        let file_name = format!("test.{}", format.extension());
        let file_path = temp_dir.path().join(&file_name);

        self.generate_uncached(format, metadata, &file_path)?;

        Ok((temp_dir, file_path))
    }

    /// Generate a test file with unicode metadata
    pub fn generate_unicode(&self, format: AudioFormat) -> Result<PathBuf, String> {
        let metadata = TestMetadata {
            title: "テストソング".to_string(),          // Japanese
            artist: "Тестовый исполнитель".to_string(), // Russian
            album: "Τεστ Άλμπουμ".to_string(),          // Greek
            genre: Some("الموسيقى".to_string()),        // Arabic
            ..Default::default()
        };

        self.generate(format, &metadata)
    }

    /// Generate an audio file with embedded artwork
    pub fn generate_with_artwork(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<PathBuf, String> {
        // Use FFmpeg to create a simple test image
        let artwork_path = self.cache_dir.join("test_artwork.png");

        if !artwork_path.exists() {
            let mut img_cmd = Command::new("ffmpeg");
            img_cmd
                .arg("-f")
                .arg("lavfi")
                .arg("-i")
                .arg("color=c=red:s=100x100:d=1")
                .arg("-frames:v")
                .arg("1")
                .arg("-y")
                .arg(&artwork_path);

            let output = img_cmd
                .output()
                .map_err(|e| format!("Failed to create artwork: {}", e))?;

            if !output.status.success() {
                return Err("Failed to generate artwork image".to_string());
            }
        }

        // Generate cache key including artwork indicator
        let mut cache_key = format!(
            "{}_{}_{}_{}_{}",
            format.extension(),
            Self::sanitize_for_filename(&metadata.title),
            Self::sanitize_for_filename(&metadata.artist),
            Self::sanitize_for_filename(&metadata.album),
            Self::sanitize_for_filename(metadata.genre.as_deref().unwrap_or("noGenre"))
        );
        cache_key.push_str("_withArt");

        // Truncate if too long
        if cache_key.len() > 200 {
            cache_key.truncate(200);
        }

        let cached_path = self
            .cache_dir
            .join(&cache_key)
            .with_extension(format.extension());

        // Return cached file if it exists
        if cached_path.exists() {
            return Ok(cached_path);
        }

        // Generate audio file with embedded artwork
        if !Self::is_ffmpeg_available() {
            return Err("FFmpeg not available".to_string());
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-i")
            .arg(&artwork_path) // Add artwork as second input
            .arg("-ac")
            .arg("2")
            .arg("-y");

        // Set sample rate
        cmd.arg("-ar").arg("44100");

        // Add codec
        cmd.arg("-codec:a").arg(format.codec());

        // Add quality settings
        match format {
            AudioFormat::Mp3 => {
                cmd.arg("-q:a").arg("2");
            }
            AudioFormat::Ogg => {
                cmd.arg("-b:a").arg("128k");
            }
            _ => {}
        }

        // Add metadata
        cmd.arg("-metadata")
            .arg(format!("title={}", metadata.title));
        cmd.arg("-metadata")
            .arg(format!("artist={}", metadata.artist));
        cmd.arg("-metadata")
            .arg(format!("album={}", metadata.album));

        if let Some(ref genre) = metadata.genre {
            cmd.arg("-metadata").arg(format!("genre={}", genre));
        }

        // Map artwork stream to metadata
        cmd.arg("-map").arg("0:a").arg("-map").arg("1:v");
        cmd.arg("-metadata:s:v").arg("title=Album cover");
        cmd.arg("-metadata:s:v").arg("comment=Cover (front)");

        // Add disposition for cover art
        cmd.arg("-disposition:v:0").arg("attached_pic");

        cmd.arg(&cached_path);

        // Execute FFmpeg
        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run FFmpeg: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFmpeg failed: {}", stderr));
        }

        Ok(cached_path)
    }
}

impl Default for FixtureGenerator {
    fn default() -> Self {
        Self::new().expect("Failed to create fixture generator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_extension() {
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Ogg.extension(), "ogg");
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_ffmpeg_available() {
        assert!(FixtureGenerator::is_ffmpeg_available());
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_generate_flac() {
        let generator = FixtureGenerator::new().unwrap();
        let metadata = TestMetadata::default();

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        assert!(path.exists());
        assert_eq!(path.extension().unwrap(), "flac");
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_generate_temp() {
        let generator = FixtureGenerator::new().unwrap();
        let metadata = TestMetadata::default();

        let (_temp_dir, path) = generator
            .generate_temp(AudioFormat::Mp3, &metadata)
            .unwrap();
        assert!(path.exists());
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_cache_reuse() {
        let generator = FixtureGenerator::new().unwrap();
        let metadata = TestMetadata::default();

        // Generate once
        let path1 = generator.generate(AudioFormat::Wav, &metadata).unwrap();
        let mtime1 = std::fs::metadata(&path1).unwrap().modified().unwrap();

        // Wait a bit
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Generate again - should return cached
        let path2 = generator.generate(AudioFormat::Wav, &metadata).unwrap();
        let mtime2 = std::fs::metadata(&path2).unwrap().modified().unwrap();

        assert_eq!(path1, path2);
        assert_eq!(mtime1, mtime2); // File wasn't regenerated
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_unicode_metadata() {
        let generator = FixtureGenerator::new().unwrap();
        let path = generator.generate_unicode(AudioFormat::Flac).unwrap();
        assert!(path.exists());
    }
}
