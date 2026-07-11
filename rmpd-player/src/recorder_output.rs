//! Recorder audio output — writes a WAV file.

use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion;
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
        }
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
        Self::write_wav_header(&mut w, self.format.sample_rate, self.format.channels)?;
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
            conversion::samples_to_s16le_into(samples, &mut self.conversion_buf);
            w.write_all(&self.conversion_buf)
                .map_err(|e| RmpdError::Player(format!("recorder write: {e}")))?;
            self.frames_written += (samples.len() / self.format.channels as usize) as u64;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        Self::finalize(&self.path, self.frames_written, self.format.channels);
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
