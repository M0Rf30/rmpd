//! Trait shared by all audio output backends.

use rmpd_core::error::Result;

/// Tracks pause state for output backends with simple flag-based pausing.
///
/// Backends that need hardware-level pause (e.g. cpal stream control) should
/// override the trait methods instead of relying on these defaults.
#[derive(Debug, Default)]
pub struct PauseState {
    paused: bool,
}

impl PauseState {
    pub fn new() -> Self {
        Self { paused: false }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

/// An audio output backend.
///
/// All methods are called from a blocking (non-async) thread.
pub trait AudioOutput: Send {
    /// Open the output device / file / pipe and prepare for playback.
    fn start(&mut self) -> Result<()>;

    /// Write interleaved f32 PCM samples (range −1.0 … +1.0).
    fn write(&mut self, samples: &[f32]) -> Result<()>;

    /// Stop playback and close the underlying resource.
    fn stop(&mut self) -> Result<()>;

    /// Access the embedded [`PauseState`].  Required for default
    /// `pause` / `resume` / `is_paused` implementations.
    fn pause_state(&self) -> &PauseState;

    /// Mutable access to the embedded [`PauseState`].
    fn pause_state_mut(&mut self) -> &mut PauseState;

    /// Whether this backend owns a real-time callback that itself applies
    /// pause-hold, flush-generation dropping, and gain (e.g. [`crate::output::CpalOutput`]
    /// via [`crate::conversion::SampleBuffer`]).
    ///
    /// `MultiOutput`'s worker uses this to decide how to treat a dequeued
    /// chunk: a self-managed backend gets every chunk forwarded unconditionally
    /// (its own callback decides what to do with pause/stale generations), while
    /// a non-self-managed backend (no real-time callback of its own — e.g. null,
    /// fifo, pipe, recorder, httpd) has pause-hold and flush-drop applied by the
    /// worker itself, and still gets the legacy write-time [`crate::filter::VolumeFilter`].
    fn self_managed(&self) -> bool {
        false
    }

    /// Pause: stop consuming samples (silence / no-op writes).
    fn pause(&mut self) -> Result<()> {
        self.pause_state_mut().set_paused(true);
        Ok(())
    }

    /// Resume after a pause.
    fn resume(&mut self) -> Result<()> {
        self.pause_state_mut().set_paused(false);
        Ok(())
    }

    /// Whether the output is currently paused.
    fn is_paused(&self) -> bool {
        self.pause_state().is_paused()
    }
}
