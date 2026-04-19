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
