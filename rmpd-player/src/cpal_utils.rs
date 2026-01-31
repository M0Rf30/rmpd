use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};
use rmpd_core::error::{Result, RmpdError};

/// CPAL device configuration helper
pub struct CpalDeviceConfig {
    pub device: Device,
    pub config: StreamConfig,
    pub sample_format: SampleFormat,
}

impl CpalDeviceConfig {
    /// Create a new device configuration with the given sample rate and channels
    pub fn new(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| RmpdError::Player("No output device available".to_owned()))?;

        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self {
            device,
            config,
            sample_format: SampleFormat::F32, // Default, will be updated by find methods
        })
    }

    /// Find the best PCM format (prefers F32, then I16, then I32)
    pub fn find_pcm_format(&mut self) -> Result<SampleFormat> {
        let preferences = &[SampleFormat::F32, SampleFormat::I16, SampleFormat::I32];
        self.find_format_with_preference(preferences, "PCM")
    }

    /// Find the best DoP format (prefers I24, then I32)
    pub fn find_dop_format(&mut self) -> Result<SampleFormat> {
        let preferences = &[SampleFormat::I24, SampleFormat::I32];
        self.find_format_with_preference(preferences, "DoP")
    }

    /// Find format matching the given preferences
    fn find_format_with_preference(
        &mut self,
        preferences: &[SampleFormat],
        format_type: &str,
    ) -> Result<SampleFormat> {
        let supported_configs = self
            .device
            .supported_output_configs()
            .map_err(|e| RmpdError::Player(format!("Failed to get supported configs: {}", e)))?;

        let mut found_format = None;
        tracing::info!(
            "Searching for suitable {} format at {:?} Hz",
            format_type,
            self.config.sample_rate
        );

        // Iterate through supported configs
        for config in supported_configs {
            let sample_format = config.sample_format();
            let min_rate = config.min_sample_rate();
            let max_rate = config.max_sample_rate();

            tracing::debug!(
                "  Checking format: {:?}, rates: {:?}-{:?} Hz",
                sample_format,
                min_rate,
                max_rate
            );

            // Check if our sample rate is supported
            if self.config.sample_rate >= min_rate && self.config.sample_rate <= max_rate {
                // Check each preference in order
                for (i, &preferred_format) in preferences.iter().enumerate() {
                    if sample_format == preferred_format {
                        // If this is the first preference, use it immediately
                        if i == 0 {
                            found_format = Some(sample_format);
                            tracing::info!(
                                "Found preferred format: {:?} at {:?}-{:?} Hz",
                                sample_format,
                                min_rate,
                                max_rate
                            );
                            break;
                        }
                        // Otherwise, only use if we haven't found a better one yet
                        else if found_format.is_none() {
                            found_format = Some(sample_format);
                            tracing::info!(
                                "Found fallback format: {:?} at {:?}-{:?} Hz",
                                sample_format,
                                min_rate,
                                max_rate
                            );
                        }
                    }
                }

                // If we found the top preference, stop searching
                if found_format == Some(preferences[0]) {
                    break;
                }
            }
        }

        let format = found_format.unwrap_or(preferences[0]);
        tracing::info!("Using sample format: {:?}", format);

        self.sample_format = format;
        Ok(format)
    }
}
