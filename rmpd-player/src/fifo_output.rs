//! Named FIFO (pipe) audio output — writes raw s16le PCM.
//!
//! Primarily used for Snapcast multi-room audio.

use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion;
use rmpd_core::error::{Result, RmpdError};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use tracing::{info, warn};

pub struct FifoOutput {
    path: String,
    writer: Option<BufWriter<std::fs::File>>,
    pause_state: PauseState,
}

impl FifoOutput {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            writer: None,
            pause_state: PauseState::new(),
        }
    }
}

impl AudioOutput for FifoOutput {
    fn start(&mut self) -> Result<()> {
        let p = std::path::Path::new(&self.path);
        if !p.exists() {
            match std::process::Command::new("mkfifo")
                .arg(&self.path)
                .status()
            {
                Ok(s) if s.success() => info!("created FIFO at {}", self.path),
                _ => warn!("mkfifo failed for {}, opening anyway", self.path),
            }
        }
        let file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|e| RmpdError::Player(format!("cannot open FIFO {}: {e}", self.path)))?;
        self.writer = Some(BufWriter::new(file));
        self.pause_state.set_paused(false);
        info!("FIFO output started: {}", self.path);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        if let Some(w) = &mut self.writer {
            let bytes = conversion::samples_to_s16le(samples);
            w.write_all(&bytes)
                .map_err(|e| RmpdError::Player(format!("FIFO write error: {e}")))?;
            w.flush()
                .map_err(|e| RmpdError::Player(format!("FIFO flush error: {e}")))?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        info!("FIFO output stopped");
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }
    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}
