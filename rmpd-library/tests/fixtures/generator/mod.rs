/// FFmpeg-based test fixture generation
///
/// This module generates minimal audio files for testing metadata extraction.
/// Files are cached in target/test-fixtures/ to avoid regenerating on each test run.
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub use rmpd_core::test_utils::AudioFormat;
use rmpd_core::test_utils::sanitize_for_filename;

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

pub struct FixtureGenerator {
    cache_dir: PathBuf,
}

impl FixtureGenerator {
    pub fn new() -> Result<Self, String> {
        let cache_dir = PathBuf::from("target/test-fixtures");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        Ok(Self { cache_dir })
    }

    pub fn is_ffmpeg_available() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    pub fn generate(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<PathBuf, String> {
        let mut cache_key = format!(
            "{}_{}_{}_{}_{}",
            format.extension(),
            sanitize_for_filename(&metadata.title),
            sanitize_for_filename(&metadata.artist),
            sanitize_for_filename(&metadata.album),
            sanitize_for_filename(metadata.genre.as_deref().unwrap_or("noGenre"))
        );

        if cache_key.len() > 200 {
            cache_key.truncate(200);
        }

        let cached_path = self
            .cache_dir
            .join(&cache_key)
            .with_extension(format.extension());

        if cached_path.exists() {
            return Ok(cached_path);
        }

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
        cmd.arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-ac")
            .arg("2")
            .arg("-y");

        cmd.arg("-ar").arg("44100");

        cmd.arg("-codec:a").arg(format.codec());

        match format {
            AudioFormat::Mp3 => {
                cmd.arg("-q:a").arg("2");
            }
            AudioFormat::Ogg => {
                cmd.arg("-b:a").arg("128k");
            }
            _ => {}
        }

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

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run FFmpeg: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFmpeg failed: {}", stderr));
        }

        Ok(())
    }

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

    pub fn generate_unicode(&self, format: AudioFormat) -> Result<PathBuf, String> {
        let metadata = TestMetadata {
            title: "テストソング".to_string(),
            artist: "Тестовый исполнитель".to_string(),
            album: "Τεστ Άλμπουμ".to_string(),
            genre: Some("الموسيقى".to_string()),
            ..Default::default()
        };

        self.generate(format, &metadata)
    }

    pub fn generate_with_artwork(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<PathBuf, String> {
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

        let mut cache_key = format!(
            "{}_{}_{}_{}_{}",
            format.extension(),
            sanitize_for_filename(&metadata.title),
            sanitize_for_filename(&metadata.artist),
            sanitize_for_filename(&metadata.album),
            sanitize_for_filename(metadata.genre.as_deref().unwrap_or("noGenre"))
        );
        cache_key.push_str("_withArt");

        if cache_key.len() > 200 {
            cache_key.truncate(200);
        }

        let cached_path = self
            .cache_dir
            .join(&cache_key)
            .with_extension(format.extension());

        if cached_path.exists() {
            return Ok(cached_path);
        }

        if !Self::is_ffmpeg_available() {
            return Err("FFmpeg not available".to_string());
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-i")
            .arg(&artwork_path)
            .arg("-ac")
            .arg("2")
            .arg("-y");

        cmd.arg("-ar").arg("44100");

        cmd.arg("-codec:a").arg(format.codec());

        match format {
            AudioFormat::Mp3 => {
                cmd.arg("-q:a").arg("2");
            }
            AudioFormat::Ogg => {
                cmd.arg("-b:a").arg("128k");
            }
            _ => {}
        }

        cmd.arg("-metadata")
            .arg(format!("title={}", metadata.title));
        cmd.arg("-metadata")
            .arg(format!("artist={}", metadata.artist));
        cmd.arg("-metadata")
            .arg(format!("album={}", metadata.album));

        if let Some(ref genre) = metadata.genre {
            cmd.arg("-metadata").arg(format!("genre={}", genre));
        }

        cmd.arg("-map").arg("0:a").arg("-map").arg("1:v");
        cmd.arg("-metadata:s:v").arg("title=Album cover");
        cmd.arg("-metadata:s:v").arg("comment=Cover (front)");

        cmd.arg("-disposition:v:0").arg("attached_pic");

        cmd.arg(&cached_path);

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
    use rmpd_core::test_utils::sanitize_for_filename;

    #[test]
    fn test_audio_format_extension() {
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Ogg.extension(), "ogg");
    }

    #[test]
    fn test_sanitize_for_filename() {
        let result = sanitize_for_filename("test/file:name");
        assert_eq!(result, "test_file_name");
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

        let path1 = generator.generate(AudioFormat::Wav, &metadata).unwrap();
        let mtime1 = std::fs::metadata(&path1).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));

        let path2 = generator.generate(AudioFormat::Wav, &metadata).unwrap();
        let mtime2 = std::fs::metadata(&path2).unwrap().modified().unwrap();

        assert_eq!(path1, path2);
        assert_eq!(mtime1, mtime2);
    }

    #[test]
    #[ignore] // Requires FFmpeg
    fn test_unicode_metadata() {
        let generator = FixtureGenerator::new().unwrap();
        let path = generator.generate_unicode(AudioFormat::Flac).unwrap();
        assert!(path.exists());
    }
}
