// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use rmpd_core::error::{Result, RmpdError};
use rmpd_player::SymphoniaDecoder;
use std::path::Path;

/// Maximum duration to fingerprint (120 seconds recommended by Chromaprint)
const MAX_FINGERPRINT_DURATION_SECS: u64 = 120;

/// Audio fingerprinter backed by `chromaprint-next`, a pure-Rust port of
/// Chromaprint producing bit-identical output to the C library.
pub struct Fingerprinter {
    inner: chromaprint::Fingerprinter,
}

impl Fingerprinter {
    /// Create a new fingerprinter instance (default algorithm, as libchromaprint)
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: chromaprint::Fingerprinter::new(chromaprint::Algorithm::default()),
        })
    }

    /// Generate a fingerprint for an audio file
    ///
    /// Returns a base64-encoded fingerprint string compatible with AcoustID.
    /// Only processes the first 120 seconds of audio as recommended by Chromaprint.
    pub fn fingerprint_file(&mut self, path: &Path) -> Result<String> {
        let lib_err =
            |what: &str, e: chromaprint::Error| RmpdError::Library(format!("{what}: {e}"));

        let mut decoder = SymphoniaDecoder::open(path)?;
        let sample_rate = decoder.sample_rate();
        let channels = decoder.channels();

        let channels_u16 = u16::try_from(channels)
            .map_err(|_| RmpdError::Library(format!("unsupported channel count {channels}")))?;
        self.inner
            .start(sample_rate, channels_u16)
            .map_err(|e| lib_err("Failed to initialize chromaprint", e))?;

        let max_samples =
            (sample_rate as u64 * channels as u64 * MAX_FINGERPRINT_DURATION_SECS) as usize;
        let mut total_samples = 0;

        let buffer_size = 4096;
        let mut f32_buffer = vec![0.0f32; buffer_size];
        let mut i16_buffer = vec![0i16; buffer_size];

        while total_samples < max_samples {
            let samples_read = match decoder.read(&mut f32_buffer) {
                Ok(n) => n,
                Err(RmpdError::Player(ref msg)) if msg.contains("end of stream") => break,
                Err(e) => return Err(e),
            };
            if samples_read == 0 {
                break;
            }

            // Convert f32 samples to i16, clamping to prevent overflow
            for (dst, &sample) in i16_buffer.iter_mut().zip(&f32_buffer[..samples_read]) {
                *dst = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            }

            self.inner
                .feed(&i16_buffer[..samples_read])
                .map_err(|e| lib_err("Failed to feed samples to chromaprint", e))?;
            total_samples += samples_read;
        }

        self.inner
            .finish()
            .map_err(|e| lib_err("Failed to finalize fingerprint", e))?;

        Ok(self.inner.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprinter_creation() {
        let fingerprinter = Fingerprinter::new();
        assert!(fingerprinter.is_ok());
    }

    #[test]
    fn test_fingerprint_nonexistent_file() {
        let mut fingerprinter = Fingerprinter::new().unwrap();
        let result = fingerprinter.fingerprint_file(Path::new("/nonexistent/file.mp3"));
        assert!(result.is_err());
    }
}
