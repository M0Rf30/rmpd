//! Trait shared by all audio output backends.

use rmpd_core::error::Result;

/// An audio output backend.
///
/// All methods are called from a blocking (non-async) thread.
pub trait AudioOutput: Send {
    /// Open the output device / file / pipe and prepare for playback.
    fn start(&mut self) -> Result<()>;

    /// Write interleaved f32 PCM samples (range −1.0 … +1.0).
    fn write(&mut self, samples: &[f32]) -> Result<()>;

    /// Pause: stop consuming samples (silence / no-op writes).
    fn pause(&mut self) -> Result<()>;

    /// Resume after a pause.
    fn resume(&mut self) -> Result<()>;

    /// Stop playback and close the underlying resource.
    fn stop(&mut self) -> Result<()>;

    /// Whether the output is currently paused.
    fn is_paused(&self) -> bool;
}
