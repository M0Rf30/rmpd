// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pluggable PCM-to-wire encoders for network audio outputs.
//!
//! Each encoder converts interleaved f32 samples to the byte format expected
//! by a particular streaming protocol.  The trait is object-safe so outputs
//! can choose an encoder at construction time.
//!
//! Encoders are selected by name through [`ENCODER_PLUGINS`] /
//! [`create_encoder`], using the `encoder`, `bitrate`, `quality` and
//! `compression` settings of an `[[output]]` block:
//!
//! | name     | feature            | settings used                         |
//! | -------- | ------------------ | ------------------------------------- |
//! | `pcm`    | —                  | —                                     |
//! | `wav`    | —                  | —                                     |
//! | `flac`   | —  (pure Rust)     | `compression` (0–8, default 5)        |

mod flac;

pub use flac::FlacEncoder;

use rmpd_core::config::OutputConfig;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;

/// Encodes interleaved f32 PCM into a wire byte stream for network outputs.
pub trait Encoder: Send {
    /// MIME type for the HTTP `Content-Type` header.
    fn content_type(&self) -> &str;

    /// Stream header bytes sent **once** to each new client on connect.
    /// Returns an empty `Vec` when no framing header is required.
    fn header(&self) -> Vec<u8>;

    /// Encode one chunk of interleaved f32 samples (−1.0 …= 1.0) to wire bytes.
    fn encode(&mut self, samples: &[f32]) -> Vec<u8>;
}

// ──────────────────────────────────────────────────────────────────────────────
// PcmEncoder
// ──────────────────────────────────────────────────────────────────────────────

/// Raw little-endian signed 16-bit PCM with no framing header.
///
/// Content-Type is `application/octet-stream`.  Suitable for Snapcast-style
/// raw-PCM consumers or as a building block for other encoders.
pub struct PcmEncoder;

impl PcmEncoder {
    pub fn new(_format: AudioFormat) -> Self {
        Self
    }
}

impl Encoder for PcmEncoder {
    fn content_type(&self) -> &str {
        "application/octet-stream"
    }

    fn header(&self) -> Vec<u8> {
        Vec::new()
    }

    fn encode(&mut self, samples: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// WavEncoder
// ──────────────────────────────────────────────────────────────────────────────

/// Streaming WAV — a canonical 44-byte RIFF/WAVE PCM-16 header followed by
/// s16le sample frames.
///
/// Both `riff_size` and `data_size` are set to `0xFFFF_FFFF` to signal an
/// unknown / streaming length.  Browsers and most media players accept this.
pub struct WavEncoder {
    format: AudioFormat,
}

impl WavEncoder {
    pub fn new(format: AudioFormat) -> Self {
        Self { format }
    }
}

impl Encoder for WavEncoder {
    fn content_type(&self) -> &str {
        "audio/wav"
    }

    fn header(&self) -> Vec<u8> {
        let channels = self.format.channels as u16;
        let sample_rate = self.format.sample_rate;
        let byte_rate: u32 = sample_rate * u32::from(channels) * 2;
        let block_align: u16 = channels * 2;
        const BITS_PER_SAMPLE: u16 = 16;
        // Use 0xFFFF_FFFF for both RIFF and data sizes — standard trick for
        // streaming WAV where the total length is not known up front.
        const STREAMING: u32 = 0xFFFF_FFFF;

        let mut h = Vec::with_capacity(44);

        // RIFF chunk descriptor (12 bytes)
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&STREAMING.to_le_bytes()); // riff_size
        h.extend_from_slice(b"WAVE");

        // "fmt " sub-chunk (24 bytes)
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
        h.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat = PCM
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&sample_rate.to_le_bytes());
        h.extend_from_slice(&byte_rate.to_le_bytes());
        h.extend_from_slice(&block_align.to_le_bytes());
        h.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

        // "data" sub-chunk header (8 bytes)
        h.extend_from_slice(b"data");
        h.extend_from_slice(&STREAMING.to_le_bytes()); // data_size

        // Total: 12 + 24 + 8 = 44 bytes
        h
    }

    fn encode(&mut self, samples: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry
// ──────────────────────────────────────────────────────────────────────────────

/// Builds an encoder for `format` from the settings of an output block.
pub type EncoderFactory = fn(AudioFormat, &OutputConfig) -> Result<Box<dyn Encoder>>;

/// Compile-time encoder registry: `encoder = "<name>"` → factory.
pub static ENCODER_PLUGINS: &[(&str, EncoderFactory)] = &[
    ("pcm", pcm_factory),
    ("wav", wav_factory),
    ("flac", flac_factory),
];

/// Name of the encoder selected by `cfg` (`encoder` setting, default `wav`).
#[must_use]
pub fn encoder_name(cfg: &OutputConfig) -> String {
    encoder_name_or(cfg, "wav")
}

/// Like [`encoder_name`] with a caller-chosen default.
#[must_use]
pub fn encoder_name_or(cfg: &OutputConfig, default: &str) -> String {
    cfg.setting_str("encoder")
        .unwrap_or_else(|| default.to_owned())
        .to_ascii_lowercase()
}

/// Names of all encoders compiled into this build.
#[must_use]
pub fn encoder_names() -> Vec<&'static str> {
    ENCODER_PLUGINS.iter().map(|(n, _)| *n).collect()
}

/// Create the encoder called `name` (case-insensitive).
///
/// # Errors
/// Returns an error for an unknown (or not compiled-in) encoder name, or when
/// the encoder rejects `format` / the settings.
pub fn create_encoder(
    name: &str,
    format: AudioFormat,
    cfg: &OutputConfig,
) -> Result<Box<dyn Encoder>> {
    let wanted = name.trim().to_ascii_lowercase();
    match ENCODER_PLUGINS.iter().find(|(n, _)| *n == wanted) {
        Some((_, factory)) => factory(format, cfg),
        None => Err(RmpdError::Player(format!(
            "unknown encoder '{name}' (available: {})",
            encoder_names().join(", ")
        ))),
    }
}

/// Create the encoder selected by the `encoder` setting of `cfg`.
///
/// # Errors
/// See [`create_encoder`].
pub fn create_encoder_from_config(
    format: AudioFormat,
    cfg: &OutputConfig,
) -> Result<Box<dyn Encoder>> {
    create_encoder(&encoder_name(cfg), format, cfg)
}

fn pcm_factory(format: AudioFormat, _cfg: &OutputConfig) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(PcmEncoder::new(format)))
}

fn wav_factory(format: AudioFormat, _cfg: &OutputConfig) -> Result<Box<dyn Encoder>> {
    Ok(Box::new(WavEncoder::new(format)))
}

fn flac_factory(format: AudioFormat, cfg: &OutputConfig) -> Result<Box<dyn Encoder>> {
    let level = setting_f64(cfg, "compression")
        .map(|v| v.clamp(0.0, f64::from(flac::MAX_COMPRESSION)) as u8)
        .unwrap_or(flac::DEFAULT_COMPRESSION);
    Ok(Box::new(FlacEncoder::new(format, level)?))
}

/// Read a numeric setting that may be written as an integer, float or string.
fn setting_f64(cfg: &OutputConfig, key: &str) -> Option<f64> {
    match cfg.settings.get(key) {
        Some(toml::Value::Integer(i)) => Some(*i as f64),
        Some(toml::Value::Float(f)) => Some(*f),
        Some(toml::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Convert one f32 sample to a rounded, clamped signed 16-bit value.
#[inline]
pub(crate) fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::AudioFormat;

    fn stereo_44100() -> AudioFormat {
        AudioFormat {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        }
    }

    // ── PcmEncoder ──────────────────────────────────────────────────────────

    #[test]
    fn pcm_positive_full_scale_is_i16_max() {
        let mut enc = PcmEncoder::new(stereo_44100());
        let bytes = enc.encode(&[1.0_f32]);
        assert_eq!(bytes.len(), 2, "one sample must produce 2 bytes");
        let v = i16::from_le_bytes([bytes[0], bytes[1]]);
        assert_eq!(v, i16::MAX); // 0x7FFF
    }

    #[test]
    fn pcm_negative_full_scale_is_minus_32767() {
        let mut enc = PcmEncoder::new(stereo_44100());
        let bytes = enc.encode(&[-1.0_f32]);
        assert_eq!(bytes.len(), 2);
        let v = i16::from_le_bytes([bytes[0], bytes[1]]);
        // (-1.0 * 32767.0) as i16 = -32767 (0x8001 in two's-complement)
        assert_eq!(v, -32767_i16);
    }

    #[test]
    fn pcm_output_length_is_samples_times_2() {
        let mut enc = PcmEncoder::new(stereo_44100());
        let samples = [0.0_f32; 128];
        assert_eq!(enc.encode(&samples).len(), 256);
    }

    #[test]
    fn pcm_header_is_empty() {
        let enc = PcmEncoder::new(stereo_44100());
        assert!(enc.header().is_empty());
    }

    // ── WavEncoder ──────────────────────────────────────────────────────────

    #[test]
    fn wav_header_is_44_bytes() {
        let enc = WavEncoder::new(stereo_44100());
        assert_eq!(enc.header().len(), 44);
    }

    #[test]
    fn wav_header_starts_with_riff() {
        let enc = WavEncoder::new(stereo_44100());
        assert_eq!(&enc.header()[0..4], b"RIFF");
    }

    #[test]
    fn wav_header_contains_wave_fmt_data_markers() {
        let enc = WavEncoder::new(stereo_44100());
        let h = enc.header();
        assert_eq!(&h[8..12], b"WAVE", "WAVE marker at offset 8");
        assert_eq!(&h[12..16], b"fmt ", "fmt  marker at offset 12");
        assert_eq!(&h[36..40], b"data", "data marker at offset 36");
    }

    #[test]
    fn wav_header_has_streaming_sizes() {
        let enc = WavEncoder::new(stereo_44100());
        let h = enc.header();
        let riff_size = u32::from_le_bytes(h[4..8].try_into().unwrap());
        let data_size = u32::from_le_bytes(h[40..44].try_into().unwrap());
        assert_eq!(riff_size, 0xFFFF_FFFF);
        assert_eq!(data_size, 0xFFFF_FFFF);
    }

    #[test]
    fn wav_encode_length_is_samples_times_2() {
        let mut enc = WavEncoder::new(stereo_44100());
        let samples = [0.0_f32; 64];
        assert_eq!(enc.encode(&samples).len(), 128);
    }

    #[test]
    fn wav_encodes_positive_full_scale() {
        let mut enc = WavEncoder::new(stereo_44100());
        let bytes = enc.encode(&[1.0_f32]);
        let v = i16::from_le_bytes([bytes[0], bytes[1]]);
        assert_eq!(v, i16::MAX);
    }

    // ── Registry ────────────────────────────────────────────────────────────

    fn cfg_with(encoder: Option<&str>) -> OutputConfig {
        let mut settings = toml::Table::new();
        if let Some(e) = encoder {
            settings.insert("encoder".into(), toml::Value::String(e.into()));
        }
        OutputConfig {
            name: "t".into(),
            output_type: "httpd".into(),
            enabled: true,
            settings,
        }
    }

    #[test]
    fn registry_always_has_core_encoders() {
        let names = encoder_names();
        for n in ["pcm", "wav", "flac"] {
            assert!(names.contains(&n), "missing {n}");
        }
    }

    #[test]
    fn registry_names_are_unique() {
        let mut names = encoder_names();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn default_encoder_is_wav() {
        let cfg = cfg_with(None);
        assert_eq!(encoder_name(&cfg), "wav");
        let enc = create_encoder_from_config(stereo_44100(), &cfg).unwrap();
        assert_eq!(enc.content_type(), "audio/wav");
        assert_eq!(enc.header().len(), 44);
    }

    #[test]
    fn create_is_case_insensitive_and_selects_by_name() {
        let cfg = cfg_with(Some("PCM"));
        let enc = create_encoder_from_config(stereo_44100(), &cfg).unwrap();
        assert_eq!(enc.content_type(), "application/octet-stream");
        assert!(enc.header().is_empty());

        let enc = create_encoder("flac", stereo_44100(), &cfg).unwrap();
        assert_eq!(enc.content_type(), "audio/flac");
        assert_eq!(&enc.header()[..4], b"fLaC");
    }

    #[test]
    fn unknown_encoder_is_an_error() {
        let err = create_encoder("mp3-hq", stereo_44100(), &cfg_with(None))
            .err()
            .expect("must fail");
        assert!(err.to_string().contains("unknown encoder"));
    }

    #[test]
    fn flac_compression_setting_is_clamped() {
        let mut cfg = cfg_with(Some("flac"));
        cfg.settings
            .insert("compression".into(), toml::Value::Integer(99));
        assert!(create_encoder_from_config(stereo_44100(), &cfg).is_ok());
    }

    #[test]
    fn setting_f64_accepts_int_float_string() {
        let mut cfg = cfg_with(None);
        cfg.settings.insert("a".into(), toml::Value::Integer(3));
        cfg.settings.insert("b".into(), toml::Value::Float(2.5));
        cfg.settings
            .insert("c".into(), toml::Value::String(" 7.5 ".into()));
        assert_eq!(setting_f64(&cfg, "a"), Some(3.0));
        assert_eq!(setting_f64(&cfg, "b"), Some(2.5));
        assert_eq!(setting_f64(&cfg, "c"), Some(7.5));
        assert_eq!(setting_f64(&cfg, "missing"), None);
    }
}
