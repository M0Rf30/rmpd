use rmpd_core::error::Result;
use rmpd_core::song::AudioFormat;

/// Audio output trait
pub trait AudioOutput {
    fn open(format: AudioFormat) -> Result<Self> where Self: Sized;
    fn write(&mut self, samples: &[f32]) -> Result<usize>;
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

// Placeholder - will implement with cpal
