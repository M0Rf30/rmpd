//! MPD-faithful audio output registry.
//!
//! Maps an `OutputConfig.output_type` string to a factory function that
//! constructs the appropriate `AudioOutput` backend.

use crate::audio_output::AudioOutput;
use crate::fifo_output::FifoOutput;
use crate::null_output::NullOutput;
use crate::output::CpalOutput;
use crate::pipe_output::PipeOutput;
use crate::recorder_output::RecorderOutput;
use rmpd_core::config::{OutputConfig, ResamplerQuality};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;

pub type OutputFactory =
    fn(AudioFormat, ResamplerQuality, &OutputConfig) -> Result<Box<dyn AudioOutput>>;

fn cpal_factory(
    format: AudioFormat,
    quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(CpalOutput::new(format, quality)?))
}

fn null_factory(
    _format: AudioFormat,
    _quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(NullOutput::new()))
}

fn fifo_factory(
    _format: AudioFormat,
    _quality: ResamplerQuality,
    cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    let path = cfg
        .setting_str("path")
        .or_else(|| cfg.setting_str("fifo_path"))
        .ok_or_else(|| {
            RmpdError::Player("fifo output requires a 'path' (or 'fifo_path') setting".into())
        })?;
    Ok(Box::new(FifoOutput::new(path)))
}

fn pipe_factory(
    _format: AudioFormat,
    _quality: ResamplerQuality,
    cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    let command = cfg
        .setting_str("command")
        .ok_or_else(|| RmpdError::Player("pipe output requires a 'command' setting".into()))?;
    Ok(Box::new(PipeOutput::new(command)))
}

fn recorder_factory(
    format: AudioFormat,
    _quality: ResamplerQuality,
    cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    let path = cfg
        .setting_str("path")
        .ok_or_else(|| RmpdError::Player("recorder output requires a 'path' setting".into()))?;
    Ok(Box::new(RecorderOutput::new(path, format)))
}

#[cfg(feature = "jack")]
fn jack_factory(
    format: AudioFormat,
    _quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(CpalOutput::new_jack(format)?))
}

#[cfg(all(feature = "asio", target_os = "windows"))]
fn asio_factory(
    format: AudioFormat,
    _quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(CpalOutput::new_asio(format)?))
}

pub static OUTPUT_PLUGINS: &[(&str, OutputFactory)] = &[
    ("cpal", cpal_factory),
    ("default", cpal_factory),
    ("null", null_factory),
    ("fifo", fifo_factory),
    ("pipe", pipe_factory),
    ("recorder", recorder_factory),
    #[cfg(feature = "jack")]
    ("jack", jack_factory),
    #[cfg(all(feature = "asio", target_os = "windows"))]
    ("asio", asio_factory),
];

pub fn create_output(
    format: AudioFormat,
    quality: ResamplerQuality,
    cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    let type_lower = cfg.output_type.to_lowercase();
    OUTPUT_PLUGINS
        .iter()
        .find(|(name, _)| *name == type_lower)
        .map(|(_, factory)| factory(format, quality, cfg))
        .unwrap_or_else(|| {
            Err(RmpdError::Player(format!(
                "unknown audio output type: {}",
                cfg.output_type
            )))
        })
}
