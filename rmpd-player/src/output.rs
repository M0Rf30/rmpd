use crate::cpal_utils::CpalDeviceConfig;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

/// cpal-based audio output
pub struct CpalOutput {
    device: Device,
    stream: Option<Stream>,
    sample_sender: Option<SyncSender<Vec<f32>>>,
    config: StreamConfig,
    is_paused: bool,
}

impl CpalOutput {
    pub fn new(format: AudioFormat) -> Result<Self> {
        let device_config = CpalDeviceConfig::new(format.sample_rate, format.channels as u16)?;

        Ok(Self {
            device: device_config.device,
            stream: None,
            sample_sender: None,
            config: device_config.config,
            is_paused: false,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(()); // Already started
        }

        // Find suitable PCM format using utility
        let mut device_config = CpalDeviceConfig {
            device: self.device.clone(),
            config: self.config.clone(),
            sample_format: SampleFormat::F32,
        };
        let sample_format = device_config.find_pcm_format()?;

        // Use bounded channel to block when buffer is full (prevents decoding faster than playback)
        // Buffer size: allow ~5 chunks to be queued (at 4096 samples/chunk, ~0.1s per chunk @ 44.1kHz)
        let (tx, rx) = sync_channel::<Vec<f32>>(5);
        let rx = Arc::new(Mutex::new(rx));
        let mut sample_buffer: Vec<f32> = Vec::new();
        let mut buffer_pos = 0;

        let rx_clone = rx.clone();

        // Build output stream based on detected format
        let stream = match sample_format {
            SampleFormat::F32 => {
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer
                            for sample in data.iter_mut() {
                                // Refill internal buffer if needed
                                if buffer_pos >= sample_buffer.len()
                                    && let Ok(rx) = rx_clone.lock()
                                    && let Ok(new_samples) = rx.try_recv()
                                {
                                    sample_buffer = new_samples;
                                    buffer_pos = 0;
                                }

                                // Output sample or silence
                                *sample = if buffer_pos < sample_buffer.len() {
                                    let val = sample_buffer[buffer_pos];
                                    buffer_pos += 1;
                                    val
                                } else {
                                    0.0
                                };
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
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer with converted samples
                            for sample in data.iter_mut() {
                                // Refill internal buffer if needed
                                if buffer_pos >= sample_buffer.len()
                                    && let Ok(rx) = rx_clone.lock()
                                    && let Ok(new_samples) = rx.try_recv()
                                {
                                    sample_buffer = new_samples;
                                    buffer_pos = 0;
                                }

                                // Output sample or silence (convert f32 to i16)
                                *sample = if buffer_pos < sample_buffer.len() {
                                    let val = sample_buffer[buffer_pos];
                                    buffer_pos += 1;
                                    // Convert F32 [-1.0, 1.0] to I16 [-32768, 32767]
                                    (val.clamp(-1.0, 1.0) * 32767.0) as i16
                                } else {
                                    0
                                };
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
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer with converted samples
                            for sample in data.iter_mut() {
                                // Refill internal buffer if needed
                                if buffer_pos >= sample_buffer.len()
                                    && let Ok(rx) = rx_clone.lock()
                                    && let Ok(new_samples) = rx.try_recv()
                                {
                                    sample_buffer = new_samples;
                                    buffer_pos = 0;
                                }

                                // Output sample or silence (convert f32 to i32)
                                *sample = if buffer_pos < sample_buffer.len() {
                                    let val = sample_buffer[buffer_pos];
                                    buffer_pos += 1;
                                    // Convert F32 [-1.0, 1.0] to I32 [-2147483648, 2147483647]
                                    (val.clamp(-1.0, 1.0) * 2147483647.0) as i32
                                } else {
                                    0
                                };
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
        self.is_paused = false;

        tracing::info!(
            "pcm output started: {:?} format, {} Hz, {} channels",
            sample_format,
            self.config.sample_rate,
            self.config.channels
        );

        Ok(())
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        if self.is_paused {
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
            self.is_paused = true;
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream
                .play()
                .map_err(|e| RmpdError::Player(format!("Failed to resume: {e}")))?;
            self.is_paused = false;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        self.sample_sender = None;
        self.is_paused = false;
        Ok(())
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused
    }
}

impl Drop for CpalOutput {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Trait for audio outputs
pub trait AudioOutput {
    fn write(&mut self, samples: &[f32]) -> Result<usize>;
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}
