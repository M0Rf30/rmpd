use rmpd_core::error::Result;
use rmpd_core::song::AudioFormat;
use std::path::Path;

/// Audio decoder trait
pub trait Decoder {
    fn open(path: &Path) -> Result<Self> where Self: Sized;
    fn read(&mut self, buffer: &mut [f32]) -> Result<usize>;
    fn seek(&mut self, position: f64) -> Result<()>;
    fn format(&self) -> AudioFormat;
    fn duration(&self) -> Option<f64>;
}

// Placeholder - will implement with Symphonia
