//! Null audio output — discards all samples.
//!
//! Used as the output backend for outputs that are disabled or when no
//! audio device is available.
//!
//! Like MPD's `null_output_plugin`, this paces writes in real time by
//! default (`sync = true`): MPD's plugin drives a `Timer` from the audio
//! format and reports the remaining delay to the player thread, so a null
//! output plays a track over its real duration instead of instantly. Without
//! pacing, a client sees the queue race to the end the moment playback
//! starts. Set `sync = false` on the output to discard samples as fast as
//! the decoder produces them.

use crate::audio_output::{AudioOutput, PauseState};
use rmpd_core::error::Result;
use rmpd_core::song::AudioFormat;
use std::time::{Duration, Instant};

/// Real-time pacer, mirroring MPD's `Timer` (`src/output/Timer.cxx`): it
/// tracks how many frames have been handed over and sleeps out the
/// difference between that playback position and the wall clock.
struct Pacer {
    frames_per_second: f64,
    channels: usize,
    started: Option<Instant>,
    frames: u64,
    /// Set while paused; `resume` shifts `started` forward by the elapsed
    /// pause duration so the target time doesn't fall permanently behind
    /// wall-clock time (PLAY-09).
    paused_at: Option<Instant>,
}

impl Pacer {
    fn new(format: AudioFormat) -> Self {
        Self {
            frames_per_second: f64::from(format.sample_rate.max(1)),
            channels: usize::from(format.channels).max(1),
            started: None,
            frames: 0,
            paused_at: None,
        }
    }

    fn add(&mut self, samples: usize) {
        let start = *self.started.get_or_insert_with(Instant::now);
        self.frames += (samples / self.channels) as u64;
        let target = Duration::from_secs_f64(self.frames as f64 / self.frames_per_second);
        if let Some(remaining) = target.checked_sub(start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }

    fn pause(&mut self) {
        if self.started.is_some() && self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take()
            && let Some(started) = self.started.as_mut()
        {
            *started += paused_at.elapsed();
        }
    }

    fn reset(&mut self) {
        self.started = None;
        self.frames = 0;
        self.paused_at = None;
    }
}

pub struct NullOutput {
    pause_state: PauseState,
    pacer: Option<Pacer>,
}

impl NullOutput {
    /// Unpaced null output: samples are dropped as fast as they arrive
    /// (MPD's `sync = false`).
    pub fn new() -> Self {
        Self {
            pause_state: PauseState::new(),
            pacer: None,
        }
    }

    /// Null output paced to `format`'s sample rate, matching MPD's default
    /// `sync = true`.
    pub fn synced(format: AudioFormat) -> Self {
        Self {
            pause_state: PauseState::new(),
            pacer: Some(Pacer::new(format)),
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

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        if let Some(pacer) = self.pacer.as_mut() {
            pacer.add(samples.len());
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(pacer) = self.pacer.as_mut() {
            pacer.reset();
        }
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }

    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }

    fn pause(&mut self) -> Result<()> {
        self.pause_state_mut().set_paused(true);
        if let Some(pacer) = self.pacer.as_mut() {
            pacer.pause();
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.pause_state_mut().set_paused(false);
        if let Some(pacer) = self.pacer.as_mut() {
            pacer.resume();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paused pacer must not count wall-clock time spent paused against its
    /// real-time budget: without resync (PLAY-09), `add` after a pause would
    /// see almost no `remaining` time and race ahead instead of pacing
    /// normally.
    #[test]
    fn pacer_resumes_without_drift_after_pause() {
        let format = AudioFormat {
            sample_rate: 1000, // 1 sample = 1ms, keeps the test fast
            channels: 1,
            bits_per_sample: 16,
        };
        let mut pacer = Pacer::new(format);

        // Prime `started` with a first (instant) add.
        pacer.add(0);

        pacer.pause();
        std::thread::sleep(Duration::from_millis(200));
        pacer.resume();

        // Immediately after resume, adding 10ms worth of frames must still
        // sleep ~10ms (not 0, which it would if the pause time had been
        // charged against the budget).
        let start = Instant::now();
        pacer.add(10);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(8),
            "pacer must resync after pause instead of racing ahead (slept {elapsed:?})"
        );
    }
}
