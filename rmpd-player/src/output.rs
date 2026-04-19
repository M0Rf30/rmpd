use crate::audio_output::{AudioOutput, PauseState};
use crate::conversion::{self, SampleBuffer};
use crate::cpal_utils::CpalDeviceConfig;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

pub struct CpalOutput {
    device: Device,
    stream: Option<Stream>,
    sample_sender: Option<SyncSender<Vec<f32>>>,
    config: StreamConfig,
    pause_state: PauseState,
}

impl CpalOutput {
    pub fn new(format: AudioFormat) -> Result<Self> {
        let device_config = CpalDeviceConfig::new(format.sample_rate, format.channels as u16)?;

        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
        })
    }

    #[cfg(feature = "jack")]
    pub fn new_jack(format: AudioFormat) -> Result<Self> {
        let device_config = CpalDeviceConfig::new_jack(format.sample_rate, format.channels as u16)?;
        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
        })
    }

    #[cfg(all(feature = "asio", target_os = "windows"))]
    pub fn new_asio(format: AudioFormat) -> Result<Self> {
        let device_config = CpalDeviceConfig::new_asio(format.sample_rate, format.channels as u16)?;
        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            pause_state: PauseState::new(),
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

        let (tx, rx) = sync_channel::<Vec<f32>>(5);
        let rx = Arc::new(Mutex::new(rx));

        let stream = match sample_format {
            SampleFormat::F32 => {
                let mut buf = SampleBuffer::new(rx.clone());
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                *sample = buf.next_sample();
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
                let mut buf = SampleBuffer::new(rx.clone());
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                *sample = conversion::f32_to_i16(buf.next_sample());
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
                let mut buf = SampleBuffer::new(rx.clone());
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                *sample = conversion::f32_to_i32(buf.next_sample());
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
        if self.pause_state.is_paused() {
            return Ok(0);
        }

        if let Some(ref sender) = self.sample_sender {
            sender
                .send(samples.to_vec())
                .map_err(|_| RmpdError::Player("Failed to send samples to output".to_owned()))?;
            Ok(samples.len())
        } else {
            Err(RmpdError::Player("Output not started".to_owned()))
        }
    }

    pub fn pause(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream
                .pause()
                .map_err(|e| RmpdError::Player(format!("Failed to pause: {e}")))?;
            self.pause_state.set_paused(true);
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream
                .play()
                .map_err(|e| RmpdError::Player(format!("Failed to resume: {e}")))?;
            self.pause_state.set_paused(false);
        }
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
}
