use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig, SampleFormat};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::sync::mpsc::{sync_channel, SyncSender};
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
        // Get default output device
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| RmpdError::Player("No output device available".to_string()))?;

        // Create stream config
        let config = StreamConfig {
            channels: format.channels as u16,
            sample_rate: format.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self {
            device,
            stream: None,
            sample_sender: None,
            config,
            is_paused: false,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(()); // Already started
        }

        // Check supported formats
        let supported_configs = self.device
            .supported_output_configs()
            .map_err(|e| RmpdError::Player(format!("Failed to get supported configs: {}", e)))?;

        // Try to find a suitable format at our sample rate
        let mut found_format = None;
        tracing::info!("Searching for suitable PCM format at {:?} Hz", self.config.sample_rate);
        for config in supported_configs {
            let sample_format = config.sample_format();
            let min_rate = config.min_sample_rate();
            let max_rate = config.max_sample_rate();

            tracing::debug!("  Checking format: {:?}, rates: {:?}-{:?} Hz", sample_format, min_rate, max_rate);

            if self.config.sample_rate >= min_rate && self.config.sample_rate <= max_rate {
                // Prefer F32, but accept I16 or I32 for hardware compatibility
                if sample_format == SampleFormat::F32 {
                    found_format = Some(sample_format);
                    tracing::info!("Found F32 format at {:?}-{:?} Hz", min_rate, max_rate);
                    break;
                } else if sample_format == SampleFormat::I16 && found_format.is_none() {
                    found_format = Some(sample_format);
                    tracing::info!("Found I16 format at {:?}-{:?} Hz", min_rate, max_rate);
                } else if sample_format == SampleFormat::I32 && found_format.is_none() {
                    found_format = Some(sample_format);
                    tracing::info!("Found I32 format at {:?}-{:?} Hz", min_rate, max_rate);
                }
            }
        }

        let sample_format = found_format.unwrap_or(SampleFormat::F32);
        tracing::info!("Using sample format: {:?}", sample_format);

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
                                if buffer_pos >= sample_buffer.len() {
                                    if let Ok(rx) = rx_clone.lock() {
                                        if let Ok(new_samples) = rx.try_recv() {
                                            sample_buffer = new_samples;
                                            buffer_pos = 0;
                                        }
                                    }
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
                            tracing::error!("PCM output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build F32 stream: {}", e)))?
            }
            SampleFormat::I16 => {
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer with converted samples
                            for sample in data.iter_mut() {
                                // Refill internal buffer if needed
                                if buffer_pos >= sample_buffer.len() {
                                    if let Ok(rx) = rx_clone.lock() {
                                        if let Ok(new_samples) = rx.try_recv() {
                                            sample_buffer = new_samples;
                                            buffer_pos = 0;
                                        }
                                    }
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
                            tracing::error!("PCM output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build I16 stream: {}", e)))?
            }
            SampleFormat::I32 => {
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer with converted samples
                            for sample in data.iter_mut() {
                                // Refill internal buffer if needed
                                if buffer_pos >= sample_buffer.len() {
                                    if let Ok(rx) = rx_clone.lock() {
                                        if let Ok(new_samples) = rx.try_recv() {
                                            sample_buffer = new_samples;
                                            buffer_pos = 0;
                                        }
                                    }
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
                            tracing::error!("PCM output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build I32 stream: {}", e)))?
            }
            _ => {
                return Err(RmpdError::Player(format!("Unsupported sample format: {:?}", sample_format)));
            }
        };

        stream
            .play()
            .map_err(|e| RmpdError::Player(format!("Failed to start stream: {}", e)))?;

        self.stream = Some(stream);
        self.sample_sender = Some(tx);
        self.is_paused = false;

        tracing::info!("PCM output started successfully");

        Ok(())
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        if self.is_paused {
            return Ok(0);
        }

        if let Some(ref sender) = self.sample_sender {
            sender
                .send(samples.to_vec())
                .map_err(|_| RmpdError::Player("Failed to send samples to output".to_string()))?;
            Ok(samples.len())
        } else {
            Err(RmpdError::Player("Output not started".to_string()))
        }
    }

    pub fn pause(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream
                .pause()
                .map_err(|e| RmpdError::Player(format!("Failed to pause: {}", e)))?;
            self.is_paused = true;
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if let Some(ref stream) = self.stream {
            stream
                .play()
                .map_err(|e| RmpdError::Player(format!("Failed to resume: {}", e)))?;
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
