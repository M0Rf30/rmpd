use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion::{self, Chunk, GainRamp, SampleBuffer};
use crate::cpal_utils::CpalDeviceConfig;
use crate::output_control::OutputControl;
use crate::resampler::StreamResampler;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rmpd_core::config::ResamplerQuality;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};

pub struct CpalOutput {
    device: Device,
    stream: Option<Stream>,
    sample_sender: Option<SyncSender<Chunk<f32>>>,
    config: StreamConfig,
    pause_state: PauseState,
    resampler: Option<StreamResampler>,
    /// Output buffer time in milliseconds; sizes the sync-channel depth.
    buffer_time_ms: u32,
    /// Shared, lock-free pause/flush/gain state. The real-time callback
    /// reads this directly — see module docs on [`crate::conversion::SampleBuffer`].
    control: Arc<OutputControl>,
}

impl CpalOutput {
    pub fn new(
        format: AudioFormat,
        quality: ResamplerQuality,
        buffer_time_ms: u32,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        Self::build(format, quality, buffer_time_ms, format.sample_rate, control)
    }

    /// Open the cpal stream at `target_device_rate` instead of
    /// `format.sample_rate`. The built-in resampler bridges the gap when they
    /// differ. Used by the DSD-to-PCM path to drive cpal at the device's native
    /// rate (e.g. 48000 Hz) rather than an advertised-but-resampled rate
    /// (e.g. 88200 Hz on a PipeWire 48 kHz graph), which prevents buffer
    /// underruns and keeps DSD ultrasonic shaped noise out of the audible band.
    pub fn with_target_rate(
        format: AudioFormat,
        quality: ResamplerQuality,
        buffer_time_ms: u32,
        target_device_rate: u32,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        Self::build(format, quality, buffer_time_ms, target_device_rate, control)
    }

    fn build(
        format: AudioFormat,
        quality: ResamplerQuality,
        buffer_time_ms: u32,
        requested_device_rate: u32,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        let device_config = CpalDeviceConfig::new(requested_device_rate, format.channels as u16)?;

        // If the device could not take the requested rate, CpalDeviceConfig
        // selected a supported one; resample to bridge the difference so the
        // file plays regardless of hardware constraints.
        let device_rate = device_config.config.sample_rate;
        let resampler = if device_rate != format.sample_rate {
            tracing::info!(
                "output device does not support {} Hz; resampling to {} Hz ({:?})",
                format.sample_rate,
                device_rate,
                quality
            );
            let rs = StreamResampler::new(
                format.sample_rate,
                device_rate,
                format.channels as usize,
                quality,
            );
            if rs.is_none() {
                tracing::error!(
                    "failed to build resampler {} -> {} Hz; audio may play at the wrong speed",
                    format.sample_rate,
                    device_rate
                );
            }
            rs
        } else {
            None
        };

        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
            resampler,
            buffer_time_ms,
            control,
        })
    }

    /// Whether the default output device natively supports `rate`. Lets callers
    /// prefer a bit-exact rate before falling back to resampling.
    pub fn supports_rate(rate: u32) -> bool {
        CpalDeviceConfig::default_device_supports_rate(rate)
    }

    /// The default output device's preferred sample rate (Hz), if known.
    pub fn default_output_rate() -> Option<u32> {
        CpalDeviceConfig::default_output_rate()
    }

    #[cfg(feature = "jack")]
    pub fn new_jack(
        format: AudioFormat,
        buffer_time_ms: u32,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        let device_config = CpalDeviceConfig::new_jack(format.sample_rate, format.channels as u16)?;
        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
            resampler: None,
            buffer_time_ms,
            control,
        })
    }

    #[cfg(all(feature = "asio", target_os = "windows"))]
    pub fn new_asio(
        format: AudioFormat,
        buffer_time_ms: u32,
        control: Arc<OutputControl>,
    ) -> Result<Self> {
        let device_config = CpalDeviceConfig::new_asio(format.sample_rate, format.channels as u16)?;
        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
            resampler: None,
            buffer_time_ms,
            control,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }

        let mut device_config = CpalDeviceConfig {
            device: self.device.clone(),
            config: self.config,
            sample_format: SampleFormat::F32,
        };
        let sample_format = device_config.find_pcm_format()?;

        // Compute channel depth from buffer_time_ms.  Each chunk sent over the
        // channel holds ~4096 samples across all channels (the engine's decode
        // loop writes BUFFER_SIZE = 4096 samples per iteration).  We divide the
        // desired buffer by the chunk size and clamp to a minimum of 4 so the
        // device callback never starves on a cold start.
        const SAMPLES_PER_CHUNK: u64 = 4096;
        let channel_depth = if self.buffer_time_ms == 0 {
            32 // safe default if somehow zero
        } else {
            let samples_needed = (self.buffer_time_ms as u64
                * self.config.sample_rate as u64
                * self.config.channels as u64)
                / 1000;
            samples_needed.div_ceil(SAMPLES_PER_CHUNK).max(4) as usize
        };
        let (tx, rx) = sync_channel::<Chunk<f32>>(channel_depth);
        let channels = self.config.channels as usize;
        let control = self.control.clone();

        let stream = match sample_format {
            SampleFormat::F32 => {
                let mut buf = SampleBuffer::new(rx, control.clone(), channels);
                let mut ramp = GainRamp::new(self.config.sample_rate, channels);
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                let raw = buf.next_sample();
                                *sample = ramp.apply(raw, control.gain());
                            }
                        },
                        |err| {
                            tracing::error!("pcm output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build F32 stream: {e}")))?
            }
            SampleFormat::I16 => {
                let mut buf = SampleBuffer::new(rx, control.clone(), channels);
                let mut ramp = GainRamp::new(self.config.sample_rate, channels);
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                let raw = buf.next_sample();
                                *sample = conversion::f32_to_i16(ramp.apply(raw, control.gain()));
                            }
                        },
                        |err| {
                            tracing::error!("pcm output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build I16 stream: {e}")))?
            }
            SampleFormat::I32 => {
                let mut buf = SampleBuffer::new(rx, control.clone(), channels);
                let mut ramp = GainRamp::new(self.config.sample_rate, channels);
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                let raw = buf.next_sample();
                                *sample = conversion::f32_to_i32(ramp.apply(raw, control.gain()));
                            }
                        },
                        |err| {
                            tracing::error!("pcm output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build I32 stream: {e}")))?
            }
            _ => {
                return Err(RmpdError::Player(format!(
                    "Unsupported sample format: {sample_format:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| RmpdError::Player(format!("Failed to start stream: {e}")))?;

        self.stream = Some(stream);
        self.sample_sender = Some(tx);
        self.pause_state.set_paused(false);

        tracing::info!(
            "pcm output started: {:?} format, {} Hz, {} channels",
            sample_format,
            self.config.sample_rate,
            self.config.channels
        );

        Ok(())
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        // Pause is handled entirely in the real-time callback (it holds
        // position and does not drain the channel); writes must keep
        // flowing so post-seek/pre-fill audio decoded while paused is ready
        // the instant playback resumes (see `SampleBuffer::next_sample`).

        // Resample to the device rate when required (bridges unsupported rates).
        let out = match &mut self.resampler {
            Some(rs) => rs.process(samples),
            None => samples.to_vec(),
        };
        let n = out.len();

        match &self.sample_sender {
            Some(sender) => {
                if n > 0 {
                    let chunk = Chunk::new(self.control.generation(), out);
                    sender.send(chunk).map_err(|_| {
                        RmpdError::Player("Failed to send samples to output".to_owned())
                    })?;
                }
                Ok(n)
            }
            None => Err(RmpdError::Player("Output not started".to_owned())),
        }
    }

    pub fn pause(&mut self) -> Result<()> {
        // Best-effort hardware-level pause; correctness never depends on
        // this succeeding (PipeWire's ALSA emulation often ignores it) — the
        // audible pause is `control.paused`, read by the real-time callback.
        if let Some(stream) = &self.stream {
            let _ = stream.pause();
        }
        self.pause_state.set_paused(true);
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if let Some(stream) = &self.stream {
            let _ = stream.play();
        }
        self.pause_state.set_paused(false);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        self.sample_sender = None;
        self.pause_state.set_paused(false);
        Ok(())
    }

    pub fn is_paused(&self) -> bool {
        self.pause_state.is_paused()
    }
}

impl Drop for CpalOutput {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl AudioOutput for CpalOutput {
    fn start(&mut self) -> rmpd_core::error::Result<()> {
        CpalOutput::start(self)
    }
    fn write(&mut self, samples: &[f32]) -> rmpd_core::error::Result<()> {
        CpalOutput::write(self, samples).map(|_| ())
    }
    fn stop(&mut self) -> rmpd_core::error::Result<()> {
        CpalOutput::stop(self)
    }
    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }
    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
    fn pause(&mut self) -> rmpd_core::error::Result<()> {
        CpalOutput::pause(self)
    }
    fn resume(&mut self) -> rmpd_core::error::Result<()> {
        CpalOutput::resume(self)
    }
    fn is_paused(&self) -> bool {
        CpalOutput::is_paused(self)
    }
    fn self_managed(&self) -> bool {
        // The real-time callback (via SampleBuffer + GainRamp) owns
        // pause-hold, flush-generation dropping, and gain directly from
        // `control`; MultiOutput must forward chunks unconditionally.
        true
    }
}
