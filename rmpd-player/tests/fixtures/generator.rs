/// FFmpeg-based test fixture generator for decoder tests
///
/// Generates minimal audio files with known patterns for validating decoder output.
/// Caches generated files to avoid regenerating on every test run.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Audio format for test fixtures
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Flac,
    Mp3,
    Ogg,
    Opus,
    M4a,
    Wav,
}

impl AudioFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Opus => "opus",
            AudioFormat::M4a => "m4a",
            AudioFormat::Wav => "wav",
        }
    }

    pub fn codec(&self) -> &'static str {
        match self {
            AudioFormat::Flac => "flac",
            AudioFormat::Mp3 => "libmp3lame",
            AudioFormat::Ogg => "libvorbis",
            AudioFormat::Opus => "libopus",
            AudioFormat::M4a => "aac",
            AudioFormat::Wav => "pcm_s16le",
        }
    }
}

/// Test audio pattern metadata
#[derive(Debug, Clone)]
pub struct TestMetadata {
    pub pattern: String,      // "sine_440hz", "impulse", "silence"
    pub sample_rate: u32,     // 44100, 48000, 96000, etc.
    pub channels: u8,         // 1 or 2
    pub duration_secs: f32,   // Duration in seconds
    pub bits_per_sample: u8,  // 16, 24, 32 (for WAV/FLAC)
}

impl Default for TestMetadata {
    fn default() -> Self {
        Self {
            pattern: "sine_1000hz".to_string(),
            sample_rate: 44100,
            channels: 2,
            duration_secs: 1.0,
            bits_per_sample: 16,
        }
    }
}

pub struct FixtureGenerator {
    cache_dir: PathBuf,
}

impl FixtureGenerator {
    /// Create a new fixture generator with caching
    pub fn new() -> Result<Self, String> {
        // Check if FFmpeg is available
        if !Self::is_ffmpeg_available() {
            return Err("FFmpeg not found. Install FFmpeg to generate test fixtures.".to_string());
        }

        // Use target/test-fixtures for caching
        let cache_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/test-fixtures/player");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Failed to create cache directory: {e}"))?;

        Ok(Self { cache_dir })
    }

    fn is_ffmpeg_available() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
    }

    /// Generate a test audio file with the specified pattern
    pub fn generate(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
    ) -> Result<PathBuf, String> {
        // Create cache key
        let cache_key = format!(
            "{}_{}_{}hz_{}ch_{:.1}s_{}bit",
            format.extension(),
            Self::sanitize_for_filename(&metadata.pattern),
            metadata.sample_rate,
            metadata.channels,
            metadata.duration_secs,
            metadata.bits_per_sample
        );

        let cache_path = self.cache_dir.join(format!("{}.{}", cache_key, format.extension()));

        // Return cached file if it exists
        if cache_path.exists() {
            return Ok(cache_path);
        }

        // Generate new file
        self.generate_file(format, metadata, &cache_path)?;

        Ok(cache_path)
    }

    fn sanitize_for_filename(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                ' ' => '_',
                c => c,
            })
            .collect()
    }

    fn generate_file(
        &self,
        format: AudioFormat,
        metadata: &TestMetadata,
        output_path: &Path,
    ) -> Result<(), String> {
        // Build FFmpeg command based on pattern
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y"); // Overwrite output file

        // Input source based on pattern
        if metadata.pattern.starts_with("sine_") {
            // Extract frequency from pattern (e.g., "sine_440hz" -> 440)
            let freq: u32 = metadata.pattern
                .trim_start_matches("sine_")
                .trim_end_matches("hz")
                .parse()
                .map_err(|_| format!("Invalid sine pattern: {}", metadata.pattern))?;

            cmd.arg("-f").arg("lavfi");
            cmd.arg("-i").arg(format!(
                "sine=frequency={}:duration={}",
                freq, metadata.duration_secs
            ));
        } else if metadata.pattern == "silence" {
            cmd.arg("-f").arg("lavfi");
            cmd.arg("-i").arg(format!(
                "anullsrc=duration={}",
                metadata.duration_secs
            ));
        } else {
            return Err(format!("Unsupported pattern: {}", metadata.pattern));
        }

        // Sample rate (format-specific)
        let sample_rate = match format {
            AudioFormat::Opus => "48000", // Opus requires 48kHz
            _ => &metadata.sample_rate.to_string(),
        };
        cmd.arg("-ar").arg(sample_rate);

        // Channels
        cmd.arg("-ac").arg(metadata.channels.to_string());

        // Format-specific encoding options
        match format {
            AudioFormat::Flac => {
                cmd.arg("-sample_fmt").arg(match metadata.bits_per_sample {
                    16 => "s16",
                    24 => "s32", // FLAC uses s32 for 24-bit
                    32 => "s32",
                    _ => "s16",
                });
                cmd.arg("-compression_level").arg("5");
            }
            AudioFormat::Mp3 => {
                cmd.arg("-codec:a").arg("libmp3lame");
                cmd.arg("-q:a").arg("2"); // VBR quality
            }
            AudioFormat::Ogg => {
                cmd.arg("-codec:a").arg("libvorbis");
                cmd.arg("-q:a").arg("5"); // VBR quality
            }
            AudioFormat::Opus => {
                cmd.arg("-codec:a").arg("libopus");
                cmd.arg("-b:a").arg("128k");
            }
            AudioFormat::M4a => {
                cmd.arg("-codec:a").arg("aac");
                cmd.arg("-b:a").arg("192k");
            }
            AudioFormat::Wav => {
                cmd.arg("-codec:a").arg(match metadata.bits_per_sample {
                    16 => "pcm_s16le",
                    24 => "pcm_s24le",
                    32 => "pcm_s32le",
                    _ => "pcm_s16le",
                });
            }
        }

        cmd.arg(output_path);

        // Execute FFmpeg
        let output = cmd
            .output()
            .map_err(|e| format!("Failed to execute FFmpeg: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("FFmpeg failed: {stderr}"));
        }

        Ok(())
    }

    /// Generate a high-resolution audio file (24-bit, 96kHz)
    pub fn generate_high_res(&self, format: AudioFormat) -> Result<PathBuf, String> {
        let metadata = TestMetadata {
            pattern: "sine_1000hz".to_string(),
            sample_rate: 96000,
            channels: 2,
            duration_secs: 1.0,
            bits_per_sample: 24,
        };
        self.generate(format, &metadata)
    }

    /// Generate a mono file
    pub fn generate_mono(&self, format: AudioFormat) -> Result<PathBuf, String> {
        let metadata = TestMetadata {
            pattern: "sine_440hz".to_string(),
            sample_rate: 44100,
            channels: 1,
            duration_secs: 1.0,
            bits_per_sample: 16,
        };
        self.generate(format, &metadata)
    }

    /// Generate a file with specific frequency
    pub fn generate_sine(&self, format: AudioFormat, frequency: u32) -> Result<PathBuf, String> {
        let metadata = TestMetadata {
            pattern: format!("sine_{}hz", frequency),
            sample_rate: 44100,
            channels: 2,
            duration_secs: 1.0,
            bits_per_sample: 16,
        };
        self.generate(format, &metadata)
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_extensions() {
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Opus.extension(), "opus");
    }

    #[test]
    fn test_sanitize_filename() {
        let input = "sine/440hz:test";
        let output = FixtureGenerator::sanitize_for_filename(input);
        assert_eq!(output, "sine_440hz_test");
    }
}
