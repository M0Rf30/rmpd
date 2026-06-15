//! Null audio output — silently discards all samples.
//!
//! Used as the output backend for outputs that are disabled or when no
//! audio device is available.

use crate::audio_output::{AudioOutput, PauseState};
use rmpd_core::error::Result;

pub struct NullOutput {
    pause_state: PauseState,
}

impl NullOutput {
    pub fn new() -> Self {
        Self {
            pause_state: PauseState::new(),
        }
    }
}

impl Default for NullOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput for NullOutput {
    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn write(&mut self, _samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }

    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}
