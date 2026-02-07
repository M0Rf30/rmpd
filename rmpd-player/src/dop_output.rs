/// DoP-specific audio output using integer samples
/// DoP requires exact bit patterns, so we use I32 format instead of F32
use crate::cpal_utils::CpalDeviceConfig;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
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
        let device_config = CpalDeviceConfig::new(sample_rate, channels as u16)?;

        // NOTE: For DoP to work properly, the device must run at the exact sample rate.
        // If PipeWire/PulseAudio is running, it may resample the audio which destroys DoP markers.
        // To fix: either configure PipeWire to pass through the sample rate,
        // or temporarily stop PipeWire: systemctl --user stop pipewire pipewire-pulse
        tracing::warn!("DoP requires direct hardware access at {} Hz, ensure PipeWire/PulseAudio isn't resampling", sample_rate);

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

        // Find suitable DoP format using utility
        let mut device_config = CpalDeviceConfig {
            device: self.device.clone(),
            config: self.config.clone(),
            sample_format: SampleFormat::I32,
        };
        let sample_format = device_config.find_dop_format()?;
        tracing::info!("requested sample rate: {:?} Hz", self.config.sample_rate);
        tracing::info!("requested channels: {}", self.config.channels);

        // Create channel for sample data
        // Increased buffer depth to handle high sample rates (DSD128/256)
        // At 352.8 kHz, we need more buffer to avoid blocking during primer
        let (tx, rx) = sync_channel::<Vec<i32>>(32);
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
                    .map_err(|e| RmpdError::Player(format!("Failed to build DoP stream: {e}")))?
            }
            _ => {
                // Fallback: convert I32 to the native format
                // This is less ideal but ensures compatibility
                tracing::warn!("no I32 format available, using fallback conversion");
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
                    .map_err(|e| RmpdError::Player(format!("Failed to build DoP stream: {e}")))?
            }
        };

        stream
            .play()
            .map_err(|e| RmpdError::Player(format!("Failed to start DoP stream: {e}")))?;

        self.stream = Some(stream);
        self.sample_sender = Some(tx.clone());
        self.is_paused = false;

        tracing::info!("DoP output started");

        // Give the stream a moment to fully initialize before sending primer
        std::thread::sleep(std::time::Duration::from_millis(50));

        // CRITICAL FIX: Prime the DAC with DoP-marked silence
        // This ensures the DAC detects DoP format immediately (blue LED)
        // Without this, first playback sounds like PCM (yellow LED)
        tracing::info!("priming DAC with DoP-marked silence for format detection");

        // Scale primer duration based on sample rate to avoid blocking
        // DSD64 (176.4kHz): 200ms, DSD128 (352.8kHz): 100ms, DSD256+: 50ms
        let sample_rate_hz = self.config.sample_rate as usize;
        let primer_duration_ms = if sample_rate_hz <= 200000 {
            200 // DSD64 and below
        } else if sample_rate_hz <= 400000 {
            100 // DSD128
        } else {
            50 // DSD256 and above
        };
        let primer_frames = (sample_rate_hz * primer_duration_ms) / 1000;
        let mut primer_samples = Vec::with_capacity(primer_frames * self.config.channels as usize);

        // Generate DoP silence: alternating markers (0x05/0xFA) with zero audio data
        // The marker must alternate per frame (not per sample) for proper DoP detection
        for frame in 0..primer_frames {
            let marker = if frame % 2 == 0 { 0x05 } else { 0xFA };
            for _ in 0..self.config.channels {
                // DoP silence: [marker][0x00][0x00][0x00] (marker in MSB)
                let dop_silence = (marker as i32) << 24;
                primer_samples.push(dop_silence);
            }
        }

        // Send primer data in smaller chunks to avoid blocking
        let chunk_size = self.config.sample_rate as usize / 50 * self.config.channels as usize; // ~20ms chunks
        for chunk in primer_samples.chunks(chunk_size) {
            tx.send(chunk.to_vec())
                .map_err(|e| RmpdError::Player(format!("Failed to send DoP primer chunk: {e}")))?;
        }

        tracing::info!(
            "DoP primer sent ({} frames = {}ms)",
            primer_frames,
            (primer_frames * 1000) / self.config.sample_rate as usize
        );

        Ok(())
    }

    /// Write DoP samples (32-bit integers) to output
    pub fn write(&mut self, samples: &[i32]) -> Result<usize> {
        if self.is_paused {
            return Ok(0);
        }

        if let Some(ref sender) = self.sample_sender {
            sender.send(samples.to_vec()).map_err(|_| {
                RmpdError::Player("Failed to send DoP samples to output".to_owned())
            })?;
            Ok(samples.len())
        } else {
            Err(RmpdError::Player("DoP output not started".to_owned()))
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
        // CRITICAL: Send PCM reset sequence to switch DAC back to PCM mode
        // This allows non-DSD tracks to play correctly after DSD playback
        tracing::info!("sending PCM reset sequence to switch DAC back to PCM mode");

        if let Some(ref sender) = self.sample_sender {
            let reset_frames = self.config.sample_rate as usize / 10; // 100ms
            let mut reset_samples =
                Vec::with_capacity(reset_frames * self.config.channels as usize);

            // Send pure silence WITHOUT DoP markers (0x00000000)
            // This signals to the DAC that we're back in regular PCM mode
            for _ in 0..(reset_frames * self.config.channels as usize) {
                reset_samples.push(0); // Plain PCM silence, no DoP markers
            }

            // Send reset data (ignore errors if channel closed)
            let _ = sender.send(reset_samples);

            // Give DAC time to process reset sequence
            std::thread::sleep(std::time::Duration::from_millis(150));

            tracing::info!("PCM reset sequence sent");
        }

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
