//! Pipe (external-command) audio output — writes raw s16le PCM to stdin.

use crate::audio_output::AudioOutput;
use rmpd_core::error::{Result, RmpdError};
use std::io::{BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use tracing::info;

pub struct PipeOutput {
    command: String,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    is_paused: bool,
}

impl PipeOutput {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            child: None,
            stdin: None,
            is_paused: false,
        }
    }

    fn samples_to_s16le(samples: &[f32]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf
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
        self.is_paused = false;
        info!("pipe output started: {}", self.command);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused {
            return Ok(());
        }
        if let Some(w) = &mut self.stdin {
            let bytes = Self::samples_to_s16le(samples);
            w.write_all(&bytes)
                .map_err(|e| RmpdError::Player(format!("pipe write error: {e}")))?;
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.is_paused = true;
        Ok(())
    }
    fn resume(&mut self) -> Result<()> {
        self.is_paused = false;
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

    fn is_paused(&self) -> bool {
        self.is_paused
    }
}
