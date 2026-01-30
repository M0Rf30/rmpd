/// DoP-specific audio output using integer samples
/// DoP requires exact bit patterns, so we use I32 format instead of F32

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig, SampleFormat};
use rmpd_core::error::{Result, RmpdError};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};

/// DoP audio output using 32-bit integer samples
pub struct DopOutput {
    device: Device,
    stream: Option<Stream>,
    sample_sender: Option<SyncSender<Vec<i32>>>,
    config: StreamConfig,
    is_paused: bool,
}

impl DopOutput {
    pub fn new(sample_rate: u32, channels: u8) -> Result<Self> {
        // For DoP, use the default device which should be configured as hw:CARD in ~/.asoundrc
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| RmpdError::Player("No output device available".to_string()))?;

        tracing::info!("Using default ALSA device (should be configured as hw: device)");

        // Create stream config for DoP
        let config = StreamConfig {
            channels: channels as u16,
            sample_rate: sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        tracing::info!("DoP output config: {}Hz, {} channels", sample_rate, channels);

        // NOTE: For DoP to work properly, the device must run at the exact sample rate.
        // If PipeWire/PulseAudio is running, it may resample the audio which destroys DoP markers.
        // To fix: either configure PipeWire to pass through the sample rate,
        // or temporarily stop PipeWire: systemctl --user stop pipewire pipewire-pulse
        tracing::warn!("DoP requires direct hardware access at {} Hz - ensure PipeWire/PulseAudio isn't resampling", sample_rate);

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

        // Try to find I32 or I24 format at our sample rate
        let mut found_format = None;
        tracing::info!("Searching for suitable format at {:?} Hz:", self.config.sample_rate);
        for config in supported_configs {
            let sample_format = config.sample_format();
            let min_rate = config.min_sample_rate();
            let max_rate = config.max_sample_rate();

            tracing::info!("  Checking format: {:?}, rates: {:?}-{:?} Hz", sample_format, min_rate, max_rate);

            if self.config.sample_rate >= min_rate && self.config.sample_rate <= max_rate {
                // Prefer I24 over I32 for DoP (24-bit format is more standard for DoP)
                if sample_format == SampleFormat::I24 {
                    found_format = Some(sample_format);
                    tracing::info!("Found suitable format: {:?} at {:?}-{:?} Hz", sample_format, min_rate, max_rate);
                    break;
                } else if sample_format == SampleFormat::I32 && found_format.is_none() {
                    found_format = Some(sample_format);
                    tracing::info!("Found suitable format: {:?} at {:?}-{:?} Hz", sample_format, min_rate, max_rate);
                }
            }
        }

        let sample_format = found_format.unwrap_or(SampleFormat::I32);
        tracing::info!("Using sample format: {:?}", sample_format);
        tracing::info!("Requested sample rate: {:?} Hz", self.config.sample_rate);
        tracing::info!("Requested channels: {}", self.config.channels);

        // Create channel for sample data
        let (tx, rx) = sync_channel::<Vec<i32>>(5);
        let rx = Arc::new(Mutex::new(rx));
        let mut sample_buffer: Vec<i32> = Vec::new();
        let mut buffer_pos = 0;

        let rx_clone = rx.clone();

        // Build output stream with integer samples (I32 or I24)
        let stream = match sample_format {
            SampleFormat::I32 | SampleFormat::I24 => {
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            // Fill output buffer with DoP samples
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

                                // Output DoP sample or silence
                                *sample = if buffer_pos < sample_buffer.len() {
                                    let val = sample_buffer[buffer_pos];
                                    buffer_pos += 1;
                                    val
                                } else {
                                    0
                                };
                            }
                        },
                        |err| {
                            tracing::error!("DoP output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build DoP stream: {}", e)))?
            }
            _ => {
                // Fallback: convert I32 to the native format
                // This is less ideal but ensures compatibility
                tracing::warn!("No I32 format available, using fallback conversion");
                self.device
                    .build_output_stream(
                        &self.config,
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                if buffer_pos >= sample_buffer.len() {
                                    if let Ok(rx) = rx_clone.lock() {
                                        if let Ok(new_samples) = rx.try_recv() {
                                            sample_buffer = new_samples;
                                            buffer_pos = 0;
                                        }
                                    }
                                }

                                *sample = if buffer_pos < sample_buffer.len() {
                                    let val = sample_buffer[buffer_pos];
                                    buffer_pos += 1;
                                    // Convert I32 DoP sample to F32
                                    // This might not work perfectly for DoP!
                                    (val as f32) / 2147483648.0
                                } else {
                                    0.0
                                };
                            }
                        },
                        |err| {
                            tracing::error!("DoP output error: {}", err);
                        },
                        None,
                    )
                    .map_err(|e| RmpdError::Player(format!("Failed to build DoP stream: {}", e)))?
            }
        };

        stream
            .play()
            .map_err(|e| RmpdError::Player(format!("Failed to start DoP stream: {}", e)))?;

        self.stream = Some(stream);
        self.sample_sender = Some(tx);
        self.is_paused = false;

        tracing::info!("DoP output started successfully");
        Ok(())
    }

    /// Write DoP samples (32-bit integers) to output
    pub fn write(&mut self, samples: &[i32]) -> Result<usize> {
        if self.is_paused {
            return Ok(0);
        }

        if let Some(ref sender) = self.sample_sender {
            sender
                .send(samples.to_vec())
                .map_err(|_| RmpdError::Player("Failed to send DoP samples to output".to_string()))?;
            Ok(samples.len())
        } else {
            Err(RmpdError::Player("DoP output not started".to_string()))
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

impl Drop for DopOutput {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
