// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Recorder audio output — writes a file.
//!
//! By default (no `encoder` setting, or `encoder = "wav"`) it writes a
//! canonical WAV file whose RIFF/data sizes are patched when recording stops.
//! Any other encoder from [`crate::encoder::ENCODER_PLUGINS`] (`flac`, `pcm`,
//! `opus` — write a `.opus` file) is streamed to the file as-is.

use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion;
use crate::encoder::Encoder;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use tracing::info;

pub struct RecorderOutput {
    path: String,
    format: AudioFormat,
    writer: Option<BufWriter<File>>,
    frames_written: u64,
    pause_state: PauseState,
    conversion_buf: Vec<u8>,
    /// `Some` for non-WAV recordings; `None` keeps the legacy WAV path.
    encoder: Option<Box<dyn Encoder>>,
}

impl RecorderOutput {
    pub fn new(path: impl Into<String>, format: AudioFormat) -> Self {
        Self {
            path: path.into(),
            format,
            writer: None,
            frames_written: 0,
            pause_state: PauseState::new(),
            conversion_buf: Vec::new(),
            encoder: None,
        }
    }

    /// Record through `encoder` instead of the built-in WAV writer.
    pub fn with_encoder(
        path: impl Into<String>,
        format: AudioFormat,
        encoder: Box<dyn Encoder>,
    ) -> Self {
        let mut out = Self::new(path, format);
        out.encoder = Some(encoder);
        out
    }

    fn write_wav_header(w: &mut BufWriter<File>, sample_rate: u32, channels: u8) -> Result<()> {
        let bps: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bps as u32 / 8;
        let block_align = channels as u16 * bps / 8;

        let e = |e: std::io::Error| RmpdError::Player(e.to_string());
        w.write_all(b"RIFF").map_err(e)?;
        w.write_all(&0u32.to_le_bytes()).map_err(e)?;
        w.write_all(b"WAVE").map_err(e)?;
        w.write_all(b"fmt ").map_err(e)?;
        w.write_all(&16u32.to_le_bytes()).map_err(e)?;
        w.write_all(&1u16.to_le_bytes()).map_err(e)?;
        w.write_all(&(channels as u16).to_le_bytes()).map_err(e)?;
        w.write_all(&sample_rate.to_le_bytes()).map_err(e)?;
        w.write_all(&byte_rate.to_le_bytes()).map_err(e)?;
        w.write_all(&block_align.to_le_bytes()).map_err(e)?;
        w.write_all(&bps.to_le_bytes()).map_err(e)?;
        w.write_all(b"data").map_err(e)?;
        w.write_all(&0u32.to_le_bytes()).map_err(e)?;
        Ok(())
    }

    /// Patches the RIFF and data chunk sizes in the WAV header once recording stops.
    ///
    /// WAV's classic RIFF format uses 32-bit little-endian size fields, which is a hard
    /// format limit (~4 GiB). Frame/byte counts are accumulated in `u64` to avoid silent
    /// wraparound during long/high-rate recordings, but if the final byte count still
    /// exceeds `u32::MAX` it is clamped (with a warning) rather than wrapped — this keeps
    /// the header internally consistent (if truncated) instead of corrupt. A correct fix
    /// for recordings beyond ~4 GiB of PCM data would require RF64/BWF, out of scope here.
    fn finalize(path: &str, frames: u64, channels: u8) {
        let data_bytes_u64 = frames * channels as u64 * 2;
        let riff_size_u64 = 36 + data_bytes_u64;
        let data_bytes = if data_bytes_u64 > u32::MAX as u64 {
            tracing::warn!(
                "recorder output: data size {data_bytes_u64} bytes exceeds WAV's 32-bit \
                 limit; clamping header field to u32::MAX (file content is unaffected)"
            );
            u32::MAX
        } else {
            data_bytes_u64 as u32
        };
        let riff_size = riff_size_u64.min(u32::MAX as u64) as u32;
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = f
                .seek(SeekFrom::Start(4))
                .and_then(|_| f.write_all(&riff_size.to_le_bytes()));
            let _ = f
                .seek(SeekFrom::Start(40))
                .and_then(|_| f.write_all(&data_bytes.to_le_bytes()));
        }
    }
}

impl AudioOutput for RecorderOutput {
    fn start(&mut self) -> Result<()> {
        let file = File::create(&self.path)
            .map_err(|e| RmpdError::Player(format!("cannot create {}: {e}", self.path)))?;
        let mut w = BufWriter::new(file);
        match &mut self.encoder {
            Some(enc) => {
                enc.reset();
                w.write_all(&enc.header())
                    .map_err(|e| RmpdError::Player(format!("recorder write: {e}")))?;
            }
            None => Self::write_wav_header(&mut w, self.format.sample_rate, self.format.channels)?,
        }
        self.writer = Some(w);
        self.frames_written = 0;
        self.pause_state.set_paused(false);
        info!("recorder output started: {}", self.path);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        if let Some(w) = &mut self.writer {
            if let Some(enc) = &mut self.encoder {
                w.write_all(&enc.encode(samples))
                    .map_err(|e| RmpdError::Player(format!("recorder write: {e}")))?;
            } else {
                conversion::samples_to_s16le_into(samples, &mut self.conversion_buf);
                w.write_all(&self.conversion_buf)
                    .map_err(|e| RmpdError::Player(format!("recorder write: {e}")))?;
            }
            self.frames_written += (samples.len() / self.format.channels as usize) as u64;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut w) = self.writer.take() {
            if let Some(enc) = &mut self.encoder {
                let tail = enc.finish();
                if let Err(e) = w.write_all(&tail) {
                    tracing::warn!("recorder: failed to write encoder tail: {e}");
                }
            }
            let _ = w.flush();
        }
        // Only the built-in WAV writer needs its header sizes patched.
        if self.encoder.is_none() {
            Self::finalize(&self.path, self.frames_written, self.format.channels);
        }
        info!("recorder output stopped: {}", self.path);
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }
    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{FlacEncoder, WavEncoder};

    fn fmt() -> AudioFormat {
        AudioFormat {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        }
    }

    #[test]
    fn default_recorder_writes_patched_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.wav");
        let mut rec = RecorderOutput::new(path.to_string_lossy().into_owned(), fmt());
        rec.start().unwrap();
        rec.write(&[0.0f32; 200]).unwrap();
        rec.stop().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 400);
        let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data, 400, "data size patched on stop");
    }

    #[test]
    fn flac_recorder_streams_encoder_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.flac");
        let enc = Box::new(FlacEncoder::new(fmt(), 5).unwrap());
        let mut rec = RecorderOutput::with_encoder(path.to_string_lossy().into_owned(), fmt(), enc);
        rec.start().unwrap();
        rec.write(&vec![0.0f32; 4096 * 2]).unwrap();
        rec.stop().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        assert!(bytes.len() > 42, "header plus one frame");
    }

    #[test]
    fn flac_recorder_finish_and_restart_are_clean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.flac");
        let enc = Box::new(FlacEncoder::new(fmt(), 5).unwrap());
        let mut rec = RecorderOutput::with_encoder(path.to_string_lossy().into_owned(), fmt(), enc);
        // 4096 + 10 frames: the 10-frame tail is only written by finish().
        rec.start().unwrap();
        rec.write(&vec![0.25f32; (4096 + 10) * 2]).unwrap();
        rec.stop().unwrap();
        let first = std::fs::read(&path).unwrap();
        // Restart with a short recording: no leftovers from the first run.
        rec.start().unwrap();
        rec.write(&[0.25f32; 6]).unwrap();
        rec.stop().unwrap();
        let second = std::fs::read(&path).unwrap();
        assert_eq!(&second[..4], b"fLaC");
        assert_eq!(second[42 + 4], 0, "frame numbering restarts");
        assert!(second.len() < first.len(), "no stale samples carried over");
        // 3 frames only: one short final frame after the 42-byte header.
        assert_eq!(second[42 + 2] >> 4, 7, "explicit block size code");
        assert_eq!(&second[42 + 5..42 + 7], &[0, 2], "block size - 1");
    }

    #[test]
    fn opus_recorder_writes_complete_ogg_stream() {
        use crate::encoder::{OpusEncoder, OpusSettings};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.opus");
        let enc = Box::new(OpusEncoder::new(fmt(), OpusSettings::default()).unwrap());
        let mut rec = RecorderOutput::with_encoder(path.to_string_lossy().into_owned(), fmt(), enc);
        rec.start().unwrap();
        rec.write(&vec![0.1f32; 44100 * 2]).unwrap();
        rec.stop().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
        assert!(bytes.windows(8).any(|w| w == b"OpusHead"));
        // The last Ogg page carries the end-of-stream flag.
        let last = bytes
            .windows(4)
            .rposition(|w| w == b"OggS")
            .expect("at least one page");
        assert_ne!(bytes[last + 5] & 0x04, 0, "EOS flag on final page");
    }

    #[test]
    fn wav_encoder_recorder_skips_header_patching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out2.wav");
        let enc = Box::new(WavEncoder::new(fmt()));
        let mut rec = RecorderOutput::with_encoder(path.to_string_lossy().into_owned(), fmt(), enc);
        rec.start().unwrap();
        rec.write(&[0.0f32; 8]).unwrap();
        rec.stop().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // Streaming sizes stay at 0xFFFFFFFF (not patched).
        assert_eq!(&bytes[4..8], &[0xFF; 4]);
    }
}
