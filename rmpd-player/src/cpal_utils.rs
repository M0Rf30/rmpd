use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};
use rmpd_core::error::{Result, RmpdError};

/// CPAL device configuration helper
pub struct CpalDeviceConfig {
    pub device: Device,
    pub config: StreamConfig,
    pub sample_format: SampleFormat,
}

/// Resolve the output device, honoring the `RMPD_AUDIO_DEVICE` override.
///
/// When the env var is set, select the output device whose name matches it
/// (exact match first, then case-insensitive substring). This lets you target a
/// raw ALSA hardware device such as `hw:CARD=D50s,DEV=0` to bypass
/// PipeWire/PulseAudio — required for bit-perfect DoP/DSD output, which any
/// resampling, mixing, or volume change would corrupt. Set `RMPD_LIST_DEVICES=1`
/// to log every available device name.
fn resolve_output_device(host: &cpal::Host) -> Result<Device> {
    let devices: Vec<(Device, String)> = host
        .output_devices()
        .map(|devs| {
            devs.map(|d| {
                let n = d.to_string();
                (d, n)
            })
            .collect()
        })
        .unwrap_or_default();

    if std::env::var_os("RMPD_LIST_DEVICES").is_some() {
        let names: Vec<&str> = devices.iter().map(|(_, n)| n.as_str()).collect();
        tracing::info!("available output devices: {names:?}");
    }

    if let Ok(want) = std::env::var("RMPD_AUDIO_DEVICE") {
        let want = want.trim();
        if !want.is_empty() {
            if let Some((dev, name)) = devices.iter().find(|(_, n)| n == want) {
                tracing::info!("using output device '{name}' (RMPD_AUDIO_DEVICE)");
                return Ok(dev.clone());
            }
            let lower = want.to_lowercase();
            if let Some((dev, name)) = devices
                .iter()
                .find(|(_, n)| n.to_lowercase().contains(&lower))
            {
                tracing::info!("using output device '{name}' (matched RMPD_AUDIO_DEVICE='{want}')");
                return Ok(dev.clone());
            }
            let available: Vec<&str> = devices.iter().map(|(_, n)| n.as_str()).collect();
            tracing::warn!(
                "RMPD_AUDIO_DEVICE='{want}' not found; using default device instead. \
                 Available: {available:?}"
            );
        }
    }

    host.default_output_device()
        .ok_or_else(|| RmpdError::Player("No output device available".to_owned()))
}

impl CpalDeviceConfig {
    /// Create a new device configuration with the given sample rate and channels
    pub fn new(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::default_host();
        let device = resolve_output_device(&host)?;
        tracing::debug!("cpal output device: {device}");

        // Choose a rate the device actually supports: the requested rate when
        // available, otherwise the device's default rate (always supported).
        // When they differ the caller resamples to bridge the gap, so playback
        // never fails just because the exact rate is unsupported.
        let rate = Self::device_supported_rate(&device, sample_rate);

        let config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self {
            device,
            config,
            sample_format: SampleFormat::F32, // Default, will be updated by find methods
        })
    }

    /// Return `requested` if the device supports it, else the device's default
    /// output rate (which is always supported by definition).
    fn device_supported_rate(device: &Device, requested: SampleRate) -> SampleRate {
        if Self::device_supports(device, requested) {
            return requested;
        }
        device
            .default_output_config()
            .map(|c| c.sample_rate())
            .unwrap_or(requested)
    }

    /// Whether `device` advertises support for `rate`.
    fn device_supports(device: &Device, rate: SampleRate) -> bool {
        device
            .supported_output_configs()
            .map(|configs| {
                configs
                    .into_iter()
                    .any(|c| rate >= c.min_sample_rate() && rate <= c.max_sample_rate())
            })
            .unwrap_or(false)
    }

    /// Whether the default output device natively supports `rate` (no
    /// resampling required). Used to prefer bit-exact rates.
    pub fn default_device_supports_rate(rate: SampleRate) -> bool {
        let host = cpal::default_host();
        resolve_output_device(&host)
            .map(|device| Self::device_supports(&device, rate))
            .unwrap_or(false)
    }

    /// The default output device's preferred (default) sample rate in Hz, if
    /// known. Used to size DSD-to-PCM decoding to the device instead of to the
    /// (often huge) advertised maximum.
    pub fn default_output_rate() -> Option<SampleRate> {
        let host = cpal::default_host();
        resolve_output_device(&host)
            .ok()
            .and_then(|device| device.default_output_config().ok())
            .map(|config| config.sample_rate())
    }

    /// Create a device configuration using the JACK host.
    #[cfg(feature = "jack")]
    pub fn new_jack(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Jack)
            .map_err(|e| RmpdError::Player(format!("JACK host not available: {e}")))?;
        let device = host
            .default_output_device()
            .ok_or_else(|| RmpdError::Player("No JACK output device available".to_owned()))?;
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        Ok(Self {
            device,
            config,
            sample_format: SampleFormat::F32,
        })
    }

    /// Create a device configuration using the ASIO host (Windows pro audio).
    #[cfg(all(feature = "asio", target_os = "windows"))]
    pub fn new_asio(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)
            .map_err(|e| RmpdError::Player(format!("ASIO host not available: {e}")))?;
        let device = host
            .default_output_device()
            .ok_or_else(|| RmpdError::Player("No ASIO output device available".to_owned()))?;
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        Ok(Self {
            device,
            config,
            sample_format: SampleFormat::F32,
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
            .map_err(|e| RmpdError::Player(format!("Failed to get supported configs: {e}")))?;

        let mut found_format = None;
        tracing::info!(
            "searching for suitable {} format at {:?} Hz",
            format_type,
            self.config.sample_rate
        );

        // Iterate through supported configs
        for config in supported_configs {
            let sample_format = config.sample_format();
            let min_rate = config.min_sample_rate();
            let max_rate = config.max_sample_rate();

            // Check if our sample rate is supported
            if self.config.sample_rate >= min_rate && self.config.sample_rate <= max_rate {
                // Check each preference in order
                for (i, &preferred_format) in preferences.iter().enumerate() {
                    if sample_format == preferred_format {
                        // If this is the first preference, use it immediately
                        if i == 0 {
                            found_format = Some(sample_format);
                            tracing::info!(
                                "found preferred format: {:?} at {:?}-{:?} Hz",
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
                                "found fallback format: {:?} at {:?}-{:?} Hz",
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
        tracing::info!("using sample format: {:?}", format);

        self.sample_format = format;
        Ok(format)
    }
}
