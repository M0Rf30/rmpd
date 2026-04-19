/// DoP-specific audio output using integer samples
/// DoP requires exact bit patterns, so we use I32 format instead of F32
use crate::conversion::SampleBuffer;
use crate::cpal_utils::CpalDeviceConfig;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use rmpd_core::error::{Result, RmpdError};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

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

        tracing::warn!(
            "DoP requires direct hardware access at {} Hz, ensure PipeWire/PulseAudio isn't resampling",
            sample_rate
        );

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
            return Ok(());
        }

        let mut device_config = CpalDeviceConfig {
            device: self.device.clone(),
            config: self.config,
            sample_format: SampleFormat::I32,
        };
        let sample_format = device_config.find_dop_format()?;
        tracing::info!("requested sample rate: {:?} Hz", self.config.sample_rate);
        tracing::info!("requested channels: {}", self.config.channels);

        let (tx, rx) = sync_channel::<Vec<i32>>(32);
        let rx = Arc::new(Mutex::new(rx));

        let stream = match sample_format {
            SampleFormat::I32 | SampleFormat::I24 => {
                let mut buf = SampleBuffer::new(rx.clone());
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                *sample = buf.next_sample();
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
                tracing::warn!("no I32 format available, using fallback conversion");
                let mut buf = SampleBuffer::new(rx.clone());
                self.device
                    .build_output_stream(
                        self.config,
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            for sample in data.iter_mut() {
                                let val = buf.next_sample();
                                *sample = (val as f32) / 2147483648.0;
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

        std::thread::sleep(std::time::Duration::from_millis(50));

        tracing::info!("priming DAC with DoP-marked silence for format detection");

        let sample_rate_hz = self.config.sample_rate as usize;
        let primer_duration_ms = if sample_rate_hz <= 200000 {
            200
        } else if sample_rate_hz <= 400000 {
            100
        } else {
            50
        };
        let primer_frames = (sample_rate_hz * primer_duration_ms) / 1000;
        let mut primer_samples = Vec::with_capacity(primer_frames * self.config.channels as usize);

        for frame in 0..primer_frames {
            let marker = if frame % 2 == 0 { 0x05 } else { 0xFA };
            for _ in 0..self.config.channels {
                let dop_silence = (marker as i32) << 24;
                primer_samples.push(dop_silence);
            }
        }

        let chunk_size = self.config.sample_rate as usize / 50 * self.config.channels as usize;
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
        tracing::info!("sending PCM reset sequence to switch DAC back to PCM mode");

        if let Some(ref sender) = self.sample_sender {
            let reset_frames = self.config.sample_rate as usize / 10;
            let mut reset_samples =
                Vec::with_capacity(reset_frames * self.config.channels as usize);

            for _ in 0..(reset_frames * self.config.channels as usize) {
                reset_samples.push(0);
            }

            let _ = sender.send(reset_samples);

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
