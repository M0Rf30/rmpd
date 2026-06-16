//! MPD-faithful audio output registry.
//!
//! Maps an `OutputConfig.output_type` string to a factory function that
//! constructs the appropriate `AudioOutput` backend.

use crate::audio_output::AudioOutput;
use crate::fifo_output::FifoOutput;
use crate::httpd_output::HttpdOutput;
use crate::null_output::NullOutput;
use crate::output::CpalOutput;
use crate::pipe_output::PipeOutput;
use crate::recorder_output::RecorderOutput;
use rmpd_core::config::{OutputConfig, ResamplerQuality};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;

pub type OutputFactory =
    fn(AudioFormat, ResamplerQuality, &OutputConfig) -> Result<Box<dyn AudioOutput>>;

// cpal_factory is kept for the OutputPlugins table but is not used by
// create_output (which routes cpal directly to carry buffer_time_ms).
fn cpal_factory(
    format: AudioFormat,
    quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(CpalOutput::new(format, quality, 500)?))
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
    Ok(Box::new(CpalOutput::new_jack(format, 500)?))
}

#[cfg(all(feature = "asio", target_os = "windows"))]
fn asio_factory(
    format: AudioFormat,
    _quality: ResamplerQuality,
    _cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(CpalOutput::new_asio(format, 500)?))
}

fn httpd_factory(
    format: AudioFormat,
    _quality: ResamplerQuality,
    cfg: &OutputConfig,
) -> Result<Box<dyn AudioOutput>> {
    Ok(Box::new(HttpdOutput::new(format, cfg)))
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
    ("httpd", httpd_factory),
];

pub fn create_output(
    format: AudioFormat,
    quality: ResamplerQuality,
    cfg: &OutputConfig,
    buffer_time_ms: u32,
    dsd_target_rate: Option<u32>,
) -> Result<Box<dyn AudioOutput>> {
    let type_lower = cfg.output_type.to_lowercase();
    // When the native PipeWire backend isn't compiled in, route a
    // `type = "pipewire"` output through the default cpal device so the config
    // still plays.
    #[cfg(not(feature = "pipewire"))]
    let type_lower = if type_lower == "pipewire" {
        tracing::debug!(
            "pipewire feature not built; routing output \"{}\" via the default cpal device",
            cfg.name
        );
        "default".to_owned()
    } else {
        type_lower
    };
    // Route cpal-family types directly so buffer_time_ms is forwarded.
    match type_lower.as_str() {
        "cpal" | "default" => {
            let out = match dsd_target_rate {
                Some(rate) => CpalOutput::with_target_rate(format, quality, buffer_time_ms, rate)?,
                None => CpalOutput::new(format, quality, buffer_time_ms)?,
            };
            return Ok(Box::new(out));
        }
        #[cfg(feature = "pipewire")]
        "pipewire" => {
            // PipeWire owns the graph rate: we open at the decoded rate and let
            // it follow/resample, so `dsd_target_rate` is intentionally ignored.
            return Ok(Box::new(crate::pipewire_output::PipeWireOutput::new(
                format,
                cfg,
                buffer_time_ms,
            )?));
        }
        #[cfg(feature = "jack")]
        "jack" => return Ok(Box::new(CpalOutput::new_jack(format, buffer_time_ms)?)),
        #[cfg(all(feature = "asio", target_os = "windows"))]
        "asio" => return Ok(Box::new(CpalOutput::new_asio(format, buffer_time_ms)?)),
        _ => {}
    }
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
