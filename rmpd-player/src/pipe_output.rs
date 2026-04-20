//! Pipe (external-command) audio output — writes raw s16le PCM to stdin.

use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion;
use rmpd_core::error::{Result, RmpdError};
use std::io::{BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use tracing::info;

pub struct PipeOutput {
    command: String,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    pause_state: PauseState,
    conversion_buf: Vec<u8>,
}

impl PipeOutput {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            child: None,
            stdin: None,
            pause_state: PauseState::new(),
            conversion_buf: Vec::new(),
        }
    }
}

impl AudioOutput for PipeOutput {
    fn start(&mut self) -> Result<()> {
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        if parts.is_empty() {
            return Err(RmpdError::Player("empty pipe command".to_owned()));
        }
        let mut child = Command::new(parts[0])
            .args(&parts[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| RmpdError::Player(format!("cannot spawn '{}': {e}", self.command)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RmpdError::Player("pipe has no stdin".to_owned()))?;
        self.stdin = Some(BufWriter::new(stdin));
        self.child = Some(child);
        self.pause_state.set_paused(false);
        info!("pipe output started: {}", self.command);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        if let Some(w) = &mut self.stdin {
            conversion::samples_to_s16le_into(samples, &mut self.conversion_buf);
            w.write_all(&self.conversion_buf)
                .map_err(|e| RmpdError::Player(format!("pipe write error: {e}")))?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        drop(self.stdin.take());
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
        info!("pipe output stopped");
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }
    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}
