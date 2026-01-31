use rmpd_core::error::{Result, RmpdError};
use rmpd_player::SymphoniaDecoder;
use std::path::Path;

/// Maximum duration to fingerprint (120 seconds recommended by Chromaprint)
const MAX_FINGERPRINT_DURATION_SECS: u64 = 120;

/// Audio fingerprinter using Chromaprint library
pub struct Fingerprinter {
    ctx: *mut chromaprint_sys_next::ChromaprintContext,
}

impl Fingerprinter {
    /// Create a new fingerprinter instance
    pub fn new() -> Result<Self> {
        // Use CHROMAPRINT_ALGORITHM_DEFAULT (value = 1)
        let ctx = unsafe { chromaprint_sys_next::chromaprint_new(1) };

        if ctx.is_null() {
            return Err(RmpdError::Library(
                "Failed to create chromaprint context".to_string(),
            ));
        }

        Ok(Self { ctx })
    }

    /// Generate a fingerprint for an audio file
    ///
    /// Returns a base64-encoded fingerprint string compatible with AcoustID.
    /// Only processes the first 120 seconds of audio as recommended by Chromaprint.
    pub fn fingerprint_file(&mut self, path: &Path) -> Result<String> {
        // Open audio file with Symphonia decoder
        let mut decoder = SymphoniaDecoder::open(path)?;

        // Get audio format info
        let sample_rate = decoder.sample_rate();
        let channels = decoder.channels();

        // Initialize chromaprint with audio format
        let result = unsafe {
            chromaprint_sys_next::chromaprint_start(
                self.ctx,
                sample_rate as i32,
                channels as i32,
            )
        };

        if result == 0 {
            return Err(RmpdError::Library(
                "Failed to initialize chromaprint".to_string(),
            ));
        }

        // Calculate maximum samples to process (120 seconds)
        let max_samples = (sample_rate as u64 * channels as u64 * MAX_FINGERPRINT_DURATION_SECS) as usize;
        let mut total_samples = 0;

        // Buffer for reading audio data
        let buffer_size = 4096;
        let mut f32_buffer = vec![0.0f32; buffer_size];
        let mut i16_buffer = vec![0i16; buffer_size];

        // Read and feed audio data to chromaprint
        loop {
            if total_samples >= max_samples {
                break;
            }

            // Read samples from decoder
            let samples_read = match decoder.read(&mut f32_buffer) {
                Ok(n) => n,
                Err(RmpdError::Player(ref msg)) if msg.contains("end of stream") => {
                    // Reached end of file
                    break;
                }
                Err(e) => return Err(e),
            };

            if samples_read == 0 {
                break;
            }

            // Convert f32 samples to i16 for chromaprint
            // Clamp to prevent overflow
            for (i, &sample) in f32_buffer[..samples_read].iter().enumerate() {
                let clamped = sample.clamp(-1.0, 1.0);
                i16_buffer[i] = (clamped * 32767.0) as i16;
            }

            // Feed samples to chromaprint
            let result = unsafe {
                chromaprint_sys_next::chromaprint_feed(
                    self.ctx,
                    i16_buffer.as_ptr(),
                    samples_read as i32,
                )
            };

            if result == 0 {
                return Err(RmpdError::Library(
                    "Failed to feed samples to chromaprint".to_string(),
                ));
            }

            total_samples += samples_read;
        }

        // Finalize fingerprint
        let result = unsafe { chromaprint_sys_next::chromaprint_finish(self.ctx) };

        if result == 0 {
            return Err(RmpdError::Library(
                "Failed to finalize fingerprint".to_string(),
            ));
        }

        // Use chromaprint_get_fingerprint_hash for a compact hash representation
        // or chromaprint_get_fingerprint for the compressed base64 string
        let mut fp_str: *mut std::os::raw::c_char = std::ptr::null_mut();

        let result = unsafe {
            chromaprint_sys_next::chromaprint_get_fingerprint(
                self.ctx,
                &mut fp_str,
            )
        };

        if result == 0 || fp_str.is_null() {
            return Err(RmpdError::Library(
                "Failed to get fingerprint".to_string(),
            ));
        }

        // Convert C string to Rust String
        let encoded = unsafe {
            let c_str = std::ffi::CStr::from_ptr(fp_str);
            c_str.to_string_lossy().into_owned()
        };

        // Free chromaprint memory
        unsafe {
            chromaprint_sys_next::chromaprint_dealloc(fp_str as *mut std::ffi::c_void);
        }

        Ok(encoded)
    }
}

impl Drop for Fingerprinter {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                chromaprint_sys_next::chromaprint_free(self.ctx);
            }
        }
    }
}

unsafe impl Send for Fingerprinter {}

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
