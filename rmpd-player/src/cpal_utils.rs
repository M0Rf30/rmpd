use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SampleRate, StreamConfig};
use rmpd_core::error::{Result, RmpdError};
use std::sync::RwLock;

/// Output device id configured at startup (from `audio.device`). Takes
/// precedence over the `RMPD_AUDIO_DEVICE` env var.
static OUTPUT_DEVICE: RwLock<Option<String>> = RwLock::new(None);

/// Set the preferred output device id (ALSA PCM name, e.g. `hw:CARD=1,DEV=0`)
/// from configuration. `None`/empty selects the system default device.
pub fn set_output_device(device: Option<String>) {
    let cleaned = device
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if let Ok(mut guard) = OUTPUT_DEVICE.write() {
        *guard = cleaned;
    }
}

/// The configured output device id: the config value first, then the
/// `RMPD_AUDIO_DEVICE` env override.
fn configured_device() -> Option<String> {
    if let Some(dev) = OUTPUT_DEVICE.read().ok().and_then(|g| g.clone()) {
        return Some(dev);
    }
    std::env::var("RMPD_AUDIO_DEVICE")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Whether an explicit output device is configured (via config or env). Used to
/// decide auto-DoP: a dedicated device implies a real (likely DoP-capable) DAC.
pub fn output_device_configured() -> bool {
    configured_device().is_some()
}

/// Expand the classic ALSA shorthand `hw:<card>,<dev>` / `hw:<card>` (and the
/// `plughw:` variants) to cpal's enumerated `hw:CARD=<card>,DEV=<dev>` form, so
/// a config value of `hw:1,0` matches the enumerated id `hw:CARD=1,DEV=0`.
/// Returns `None` when the input is not in numeric shorthand form.
fn normalize_alsa(name: &str) -> Option<String> {
    for prefix in ["plughw:", "hw:"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.starts_with("CARD=") {
                return None; // already canonical
            }
            let mut parts = rest.splitn(2, ',');
            let card = parts.next()?.trim();
            let dev = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("0");
            if card.is_empty()
                || !card.bytes().all(|b| b.is_ascii_digit())
                || !dev.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            return Some(format!("{prefix}CARD={card},DEV={dev}"));
        }
    }
    None
}

/// CPAL device configuration helper
pub struct CpalDeviceConfig {
    pub device: Device,
    pub config: StreamConfig,
    pub sample_format: SampleFormat,
}

/// Resolve the output device, honoring the `RMPD_AUDIO_DEVICE` override.
///
/// When the env var is set, select the output device whose **id** (the ALSA PCM
/// name, e.g. `hw:CARD=1,DEV=0`) or description matches it — exact match first,
/// then case-insensitive substring. This lets you target a raw ALSA hardware
/// device to bypass PipeWire/PulseAudio, which is required for bit-perfect
/// DoP/DSD output (any resampling, mixing, or volume change corrupts it).
///
/// Set `RMPD_LIST_DEVICES=1` to log every device's id + description so you can
/// pick the exact `hw:` device.
fn resolve_output_device(host: &cpal::Host) -> Result<Device> {
    // (device, id string e.g. "hw:CARD=1,DEV=0", human description)
    let devices: Vec<(Device, String, String)> = host
        .output_devices()
        .map(|devs| {
            devs.map(|d| {
                let id = d.id().map(|i| i.id().to_owned()).unwrap_or_default();
                let desc = d.to_string();
                (d, id, desc)
            })
            .collect()
        })
        .unwrap_or_default();

    if std::env::var_os("RMPD_LIST_DEVICES").is_some() {
        for (_, id, desc) in &devices {
            tracing::info!("output device: id='{id}' desc='{desc}'");
        }
    }

    if let Some(want) = configured_device() {
        let want = want.as_str();
        // 1) exact id, 2) exact desc, 3) substring id, 4) substring desc
        let lower = want.to_lowercase();
        let norm = normalize_alsa(want);
        let pick = devices
            .iter()
            .find(|(_, id, _)| id == want)
            .or_else(|| {
                norm.as_deref()
                    .and_then(|n| devices.iter().find(|(_, id, _)| id == n))
            })
            .or_else(|| devices.iter().find(|(_, _, desc)| desc == want))
            .or_else(|| {
                devices
                    .iter()
                    .find(|(_, id, _)| id.to_lowercase().contains(&lower))
            })
            .or_else(|| {
                devices
                    .iter()
                    .find(|(_, _, desc)| desc.to_lowercase().contains(&lower))
            });
        if let Some((dev, id, desc)) = pick {
            tracing::info!("using output device id='{id}' desc='{desc}' (configured '{want}')");
            return Ok(dev.clone());
        }
        let available: Vec<&str> = devices.iter().map(|(_, id, _)| id.as_str()).collect();
        tracing::warn!(
            "configured output device '{want}' not found; using default. Available ids: {available:?}"
        );
    }

    host.default_output_device()
        .ok_or_else(|| RmpdError::Player("No output device available".to_owned()))
}

/// Find a bit-perfect output device for DoP/native DSD: a raw ALSA `hw:` device
/// that natively supports the exact `rate` with at least `channels` channels, so
/// PipeWire/PulseAudio never resamples and corrupts the DoP stream.
///
/// HDMI and S/PDIF outputs are excluded — they advertise high PCM rates but are
/// not DoP DACs (auto-picking one yields silence). USB-described devices are
/// preferred, then the highest maximum rate. Returns `None` when nothing clearly
/// qualifies; the caller should fall back to PCM rather than guess. For
/// certainty (or when the DAC is briefly busy), set `audio.device` explicitly.
fn find_dop_device(host: &cpal::Host, rate: SampleRate, channels: u16) -> Option<(Device, String)> {
    // (device, id, is_usb, max_rate)
    let mut best: Option<(Device, String, bool, u32)> = None;
    for device in host.output_devices().ok()? {
        let id = device.id().map(|i| i.id().to_owned()).unwrap_or_default();
        // Only raw hardware devices give an exclusive, non-resampled path.
        if !id.starts_with("hw:") {
            continue;
        }
        let desc = device.to_string().to_lowercase();
        // HDMI/SPDIF take high PCM rates but are not DoP DACs — never auto-pick.
        if desc.contains("hdmi") || desc.contains("s/pdif") || desc.contains("iec958") {
            continue;
        }
        let mut supports = false;
        let mut max_rate = 0u32;
        if let Ok(configs) = device.supported_output_configs() {
            for c in configs {
                max_rate = max_rate.max(c.max_sample_rate());
                if c.channels() >= channels
                    && rate >= c.min_sample_rate()
                    && rate <= c.max_sample_rate()
                {
                    supports = true;
                }
            }
        }
        if !supports {
            continue;
        }
        let is_usb = desc.contains("usb");
        // Prefer a USB DAC, then the highest maximum rate.
        if best
            .as_ref()
            .is_none_or(|(_, _, u, m)| (is_usb, max_rate) > (*u, *m))
        {
            best = Some((device, id, is_usb, max_rate));
        }
    }
    best.map(|(device, id, _, _)| (device, id))
}

impl CpalDeviceConfig {
    /// Create a new device configuration with the given sample rate and channels
    pub fn new(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::default_host();
        let mut device = resolve_output_device(&host)?;

        // The configured device may be busy (held by PipeWire) or disconnected.
        // For ordinary PCM playback, degrade gracefully to the default device
        // rather than failing all playback. (DoP uses `new_dop`, which instead
        // reports the failure so the engine falls back to PCM conversion.)
        if device.supported_output_configs().is_err()
            && let Some(default) = host.default_output_device()
        {
            tracing::warn!(
                "configured output device '{device}' is unavailable/busy; \
                 falling back to the default device"
            );
            device = default;
        }
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

    /// Device configuration for DoP/native DSD at the **exact** `sample_rate`
    /// (no resampling). Auto-selects a bit-perfect `hw:` DAC that natively
    /// supports the rate (see [`find_dop_device`]), preferring an explicitly
    /// configured device when it qualifies, and falling back to the resolved
    /// device otherwise (DoP then likely fails and the caller reverts to PCM).
    pub fn new_dop(sample_rate: SampleRate, channels: u16) -> Result<Self> {
        let host = cpal::default_host();
        // An explicitly configured device is used verbatim (no auto-substitution),
        // so DoP never silently routes to the wrong output. Auto-discovery only
        // runs when no device is configured, and only over real (USB) DACs.
        let device = if configured_device().is_some() {
            let dev = resolve_output_device(&host)?;
            tracing::info!("DoP: using configured device '{dev}' at {sample_rate} Hz");
            dev
        } else {
            match find_dop_device(&host, sample_rate, channels) {
                Some((dev, id)) => {
                    tracing::info!(
                        "DoP: auto-selected bit-perfect device '{id}' at {sample_rate} Hz"
                    );
                    dev
                }
                // No real DAC: fail so the caller cleanly reverts to PCM, rather
                // than opening DoP on a wrong/HDMI/PipeWire device (silence).
                None => {
                    return Err(RmpdError::Player(format!(
                        "no USB/hardware DAC natively supports {sample_rate} Hz \
                         (HDMI/SPDIF excluded); set audio.device to your DAC"
                    )));
                }
            }
        };

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
