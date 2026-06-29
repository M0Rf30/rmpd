// Audio player engine
pub mod audio_output;
pub mod conversion;
pub mod cpal_utils;
pub mod crossfade;
pub mod decoder;
pub mod dop;
pub mod dop_output;
pub mod encoder;
pub mod engine;
pub mod fifo_output;
pub mod filter;
pub mod httpd_output;
pub mod multi_output;
pub mod null_output;
pub mod output;
pub mod output_registry;
pub mod output_slot;
pub mod pipe_output;
#[cfg(all(feature = "pipewire", target_os = "linux"))]
pub mod pipewire_output;
pub mod recorder_output;
pub mod resampler;

pub use cpal_utils::set_output_device;
pub use decoder::{
    DECODER_PLUGINS, Decoder, DecoderPlugin, SymphoniaDecoder, decoder_for_suffix,
    is_supported_suffix,
};
pub use dop::DopEncoder;
pub use encoder::{Encoder, PcmEncoder, WavEncoder};
pub use engine::PlaybackEngine;
pub use filter::{AudioFilter, FilterChain, Mixer, SoftwareMixer, VolumeFilter};
pub use httpd_output::HttpdOutput;
pub use multi_output::MultiOutput;
pub use null_output::NullOutput;
pub use output::CpalOutput;
pub use output_registry::{OUTPUT_PLUGINS, create_output};
pub use output_slot::{OutputKey, OutputSlot};
#[cfg(all(feature = "pipewire", target_os = "linux"))]
pub use pipewire_output::PipeWireOutput;
