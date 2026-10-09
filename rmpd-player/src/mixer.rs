// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mixer plugin registry and per-output mixer selection.
//!
//! MPD semantics: every `[[output]]` picks a mixer with `mixer_type`:
//!
//! | `mixer_type`      | Plugin     | Behaviour                                         |
//! |-------------------|------------|---------------------------------------------------|
//! | `software` (default) | `software` | gain applied by rmpd's own gain stage          |
//! | `hardware`        | `alsa`     | the output's native mixer (ALSA `Selem` volume)   |
//! | `none` / `null`   | `none`     | no volume control (`setvol` fails like MPD)       |
//!
//! `hardware` maps to the output's native mixer: ALSA for `cpal`/`default`/
//! `alsa` outputs on Linux, compiled in with the `alsa-mixer` feature (which
//! reuses the `alsa` crate cpal already links). Any other combination is a
//! configuration warning and falls back to the software mixer.
//!
//! Additional keys accepted next to `mixer_type` (see [`OUTPUT_MIXER_SETTINGS`]):
//! `mixer_device` (default `default`, or the card of the output's `hw:` device),
//! `mixer_control` (default: `PCM`, then `Master`) and `mixer_index` (default 0).
//!
//! The engine owns one [`MixerSet`] built from the enabled outputs. When no
//! enabled output uses the software mixer the software gain stays at unity
//! (100%), so a hardware mixer never stacks with an additional digital
//! attenuation. Factories are synchronous and perform no I/O; hardware access
//! happens when a volume is read or written.

use crate::filter::{Mixer, SoftwareMixer};
use rmpd_core::config::OutputConfig;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use thiserror::Error;
use tracing::warn;

/// Setting keys of an `[[output]]` block that select and configure its mixer.
pub const OUTPUT_MIXER_SETTINGS: &[&str] =
    &["mixer_type", "mixer_device", "mixer_control", "mixer_index"];

/// Errors raised while selecting, building or driving a mixer.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MixerError {
    #[error("unknown mixer_type `{0}` (expected software, hardware or none)")]
    UnknownType(String),
    #[error("output type `{0}` has no hardware mixer in this build")]
    NoHardwareMixer(String),
    #[error("invalid mixer_index `{0}` (expected a non-negative integer)")]
    InvalidIndex(String),
    /// The output has `mixer_type = none`: volume can be neither set nor read.
    #[error("no mixer available")]
    NoMixer,
    #[error("hardware mixer: {0}")]
    Backend(String),
}

/// The user-facing `mixer_type` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerType {
    Software,
    Hardware,
    None,
}

impl MixerType {
    /// Parse a `mixer_type` value (case-insensitive; `null` is an alias of
    /// `none`, as in MPD).
    pub fn parse(value: &str) -> Result<Self, MixerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "software" => Ok(Self::Software),
            "hardware" => Ok(Self::Hardware),
            "none" | "null" => Ok(Self::None),
            _ => Err(MixerError::UnknownType(value.trim().to_owned())),
        }
    }
}

/// Inputs handed to a [`MixerFactory`].
#[derive(Debug, Clone)]
pub struct MixerParams {
    /// Hardware mixer device (`mixer_device`), e.g. `default` or `hw:1`.
    pub device: String,
    /// Explicit `mixer_control`; `None` tries `PCM` then `Master`.
    pub control: Option<String>,
    /// `mixer_index` (ALSA simple-element index).
    pub index: u32,
    /// The engine's software gain atomic (shared with the `VolumeFilter`).
    pub software_volume: Arc<AtomicU8>,
}

pub type MixerFactory = fn(&MixerParams) -> Result<Box<dyn Mixer>, MixerError>;

/// One mixer plugin: registry name, accepted setting keys, factory.
pub struct MixerPlugin {
    pub name: &'static str,
    pub settings: &'static [&'static str],
    pub factory: MixerFactory,
}

fn software_factory(p: &MixerParams) -> Result<Box<dyn Mixer>, MixerError> {
    Ok(Box::new(SoftwareMixer::new(p.software_volume.clone())))
}

fn none_factory(_p: &MixerParams) -> Result<Box<dyn Mixer>, MixerError> {
    Ok(Box::new(NullMixer))
}

#[cfg(all(feature = "alsa-mixer", target_os = "linux"))]
fn alsa_factory(p: &MixerParams) -> Result<Box<dyn Mixer>, MixerError> {
    Ok(Box::new(alsa_backend::AlsaMixer::new(
        &p.device,
        p.control.as_deref(),
        p.index,
    )?))
}

/// Compile-time mixer registry, selected by name.
pub static MIXER_PLUGINS: &[MixerPlugin] = &[
    MixerPlugin {
        name: "software",
        settings: &[],
        factory: software_factory,
    },
    MixerPlugin {
        name: "none",
        settings: &[],
        factory: none_factory,
    },
    #[cfg(all(feature = "alsa-mixer", target_os = "linux"))]
    MixerPlugin {
        name: "alsa",
        settings: &["mixer_device", "mixer_control", "mixer_index"],
        factory: alsa_factory,
    },
];

/// Look a mixer plugin up by registry name.
pub fn plugin_by_name(name: &str) -> Option<&'static MixerPlugin> {
    MIXER_PLUGINS.iter().find(|p| p.name == name)
}

/// Output types backed by an ALSA PCM (cpal on Linux, or a plain `alsa` type).
fn is_alsa_output_type(output_type: &str) -> bool {
    matches!(
        output_type.trim().to_ascii_lowercase().as_str(),
        "cpal" | "default" | "alsa"
    )
}

/// The registry name of the native hardware mixer of an output type, if any.
fn native_hardware_plugin(output_type: &str) -> Option<&'static str> {
    if is_alsa_output_type(output_type) {
        plugin_by_name("alsa").map(|p| p.name)
    } else {
        None
    }
}

/// Pure plugin selection for one output: `mixer_type` (`None` = unset =
/// software, which keeps the pre-plugin behaviour) + the output type.
pub fn select_plugin(
    mixer_type: Option<&str>,
    output_type: &str,
) -> Result<&'static MixerPlugin, MixerError> {
    let ty = match mixer_type {
        None => MixerType::Software,
        Some(s) => MixerType::parse(s)?,
    };
    let name = match ty {
        MixerType::Software => "software",
        MixerType::None => "none",
        MixerType::Hardware => native_hardware_plugin(output_type)
            .ok_or_else(|| MixerError::NoHardwareMixer(output_type.to_owned()))?,
    };
    plugin_by_name(name).ok_or_else(|| MixerError::NoHardwareMixer(output_type.to_owned()))
}

/// Derive an ALSA control (mixer) device from a PCM name: the PCM part
/// (`DEV=..`/`,<dev>`) is dropped since a control only addresses the card.
///
/// `hw:1,0` → `hw:1`, `plughw:CARD=PCH,DEV=0` → `hw:CARD=PCH`. Returns `None`
/// for non-`hw` PCMs (`default`, `pipewire`, …).
pub fn mixer_device_for_pcm(pcm: &str) -> Option<String> {
    let (prefix, args) = pcm.trim().split_once(':')?;
    if !matches!(prefix.trim().to_ascii_lowercase().as_str(), "hw" | "plughw") {
        return None;
    }
    let card = args.split(',').next()?.trim();
    if card.is_empty() {
        return None;
    }
    Some(format!("hw:{card}"))
}

/// Simple-element names to try, in order: the explicit `mixer_control` only,
/// otherwise `PCM` then `Master`.
pub fn control_candidates(control: Option<&str>) -> Vec<String> {
    match control.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => vec![c.to_owned()],
        None => vec!["PCM".to_owned(), "Master".to_owned()],
    }
}

/// Map a percentage (0..=100) onto a hardware volume range, rounding to the
/// nearest step. A degenerate range collapses to `min`.
pub fn percent_to_raw(percent: u8, min: i64, max: i64) -> i64 {
    if max <= min {
        return min;
    }
    let p = i64::from(percent.min(100));
    min + ((max - min) * p + 50) / 100
}

/// Map a raw hardware volume back to a percentage (0..=100), rounding to the
/// nearest percent. Values outside the range are clamped; a degenerate range
/// reports 0.
pub fn raw_to_percent(raw: i64, min: i64, max: i64) -> u8 {
    if max <= min {
        return 0;
    }
    let span = max - min;
    let raw = raw.clamp(min, max);
    let pct = ((raw - min) * 100 + span / 2) / span;
    pct.clamp(0, 100) as u8
}

/// Build the mixer parameters for an output from its `mixer_*` settings.
pub fn params_for_output(
    cfg: &OutputConfig,
    software_volume: &Arc<AtomicU8>,
) -> Result<MixerParams, MixerError> {
    let index = match cfg.setting_str("mixer_index") {
        None => 0,
        Some(s) => s.parse::<u32>().map_err(|_| MixerError::InvalidIndex(s))?,
    };
    let device = cfg
        .setting_str("mixer_device")
        .or_else(|| {
            if is_alsa_output_type(&cfg.output_type) {
                cfg.setting_str("device")
                    .or_else(crate::cpal_utils::configured_output_device)
                    .and_then(|pcm| mixer_device_for_pcm(&pcm))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "default".to_owned());
    Ok(MixerParams {
        device,
        control: cfg.setting_str("mixer_control"),
        index,
        software_volume: software_volume.clone(),
    })
}

/// Build the mixer of one output (selection + factory).
pub fn build_output_mixer(
    cfg: &OutputConfig,
    software_volume: &Arc<AtomicU8>,
) -> Result<Box<dyn Mixer>, MixerError> {
    let plugin = select_plugin(cfg.setting_str("mixer_type").as_deref(), &cfg.output_type)?;
    let params = params_for_output(cfg, software_volume)?;
    (plugin.factory)(&params)
}

// ── NullMixer ────────────────────────────────────────────────────────────────

/// `mixer_type = none`: the output has no volume control.
pub struct NullMixer;

impl Mixer for NullMixer {
    fn name(&self) -> &str {
        "none"
    }

    fn controls_volume(&self) -> bool {
        false
    }

    fn set_volume(&self, _v: u8) -> Result<(), MixerError> {
        Err(MixerError::NoMixer)
    }

    fn volume(&self) -> Result<u8, MixerError> {
        Err(MixerError::NoMixer)
    }
}

// ── MixerSet ─────────────────────────────────────────────────────────────────

/// The mixers of all enabled outputs, as driven by `setvol`/`status`.
///
/// Like MPD, a volume write goes to every mixer and the reported volume is
/// the average of the mixers that can report one.
pub struct MixerSet {
    entries: Vec<Arc<dyn Mixer>>,
}

impl MixerSet {
    /// Build the set for `outputs`. Outputs whose mixer cannot be built log a
    /// warning and use the software mixer; an empty output list yields a single
    /// software mixer so `setvol` keeps working.
    pub fn from_outputs(outputs: &[OutputConfig], software_volume: &Arc<AtomicU8>) -> Self {
        let mut entries: Vec<Arc<dyn Mixer>> = Vec::with_capacity(outputs.len().max(1));
        for cfg in outputs {
            match build_output_mixer(cfg, software_volume) {
                Ok(m) => entries.push(Arc::from(m)),
                Err(e) => {
                    warn!(
                        "output \"{}\": {e}; falling back to the software mixer",
                        cfg.name
                    );
                    entries.push(Arc::new(SoftwareMixer::new(software_volume.clone())));
                }
            }
        }
        if entries.is_empty() {
            entries.push(Arc::new(SoftwareMixer::new(software_volume.clone())));
        }
        let set = Self { entries };
        if set.has_software() && set.has_hardware() {
            warn!(
                "outputs mix software and hardware mixers: the software gain is shared by \
                 all outputs, so hardware-mixer outputs are attenuated twice"
            );
        }
        set
    }

    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<Arc<dyn Mixer>>) -> Self {
        Self { entries }
    }

    /// At least one enabled output uses the software mixer.
    pub fn has_software(&self) -> bool {
        self.entries.iter().any(|m| m.is_software())
    }

    /// At least one enabled output uses a device (hardware) mixer.
    pub fn has_hardware(&self) -> bool {
        self.entries
            .iter()
            .any(|m| !m.is_software() && m.controls_volume())
    }

    /// At least one mixer can set/read a volume (`false` = all `none`).
    pub fn controls_volume(&self) -> bool {
        self.entries.iter().any(|m| m.controls_volume())
    }

    /// Set `v` percent on every volume-capable mixer. Fails when there is no
    /// such mixer or when every one of them failed; partial failures only warn.
    pub fn set_volume(&self, v: u8) -> Result<(), MixerError> {
        let mut first_err = None;
        let mut ok = 0usize;
        let mut attempted = 0usize;
        for m in self.entries.iter().filter(|m| m.controls_volume()) {
            attempted += 1;
            match m.set_volume(v) {
                Ok(()) => ok += 1,
                Err(e) => {
                    warn!("{} mixer: setting volume failed: {e}", m.name());
                    first_err.get_or_insert(e);
                }
            }
        }
        if attempted == 0 {
            return Err(MixerError::NoMixer);
        }
        if ok == 0 {
            return Err(first_err.unwrap_or(MixerError::NoMixer));
        }
        Ok(())
    }

    /// Average volume of the mixers that can report one; `None` when none can.
    pub fn volume(&self) -> Option<u8> {
        let vols: Vec<u32> = self
            .entries
            .iter()
            .filter(|m| m.controls_volume())
            .filter_map(|m| m.volume().ok())
            .map(u32::from)
            .collect();
        if vols.is_empty() {
            return None;
        }
        let sum: u32 = vols.iter().sum();
        let n = vols.len() as u32;
        Some(((sum + n / 2) / n).min(100) as u8)
    }

    /// Volume of the hardware mixers only (`None` without any).
    pub fn hardware_volume(&self) -> Option<u8> {
        let vols: Vec<u32> = self
            .entries
            .iter()
            .filter(|m| !m.is_software() && m.controls_volume())
            .filter_map(|m| m.volume().ok())
            .map(u32::from)
            .collect();
        if vols.is_empty() {
            return None;
        }
        let sum: u32 = vols.iter().sum();
        let n = vols.len() as u32;
        Some(((sum + n / 2) / n).min(100) as u8)
    }
}

// ── ALSA hardware mixer ──────────────────────────────────────────────────────

#[cfg(all(feature = "alsa-mixer", target_os = "linux"))]
mod alsa_backend {
    use super::{MixerError, control_candidates, percent_to_raw, raw_to_percent};
    use crate::filter::Mixer;
    use alsa::mixer::{Mixer as AlsaHandle, Selem, SelemChannelId, SelemId};
    use parking_lot::Mutex;
    use std::time::{Duration, Instant};

    /// How long a read volume is served from memory (status polling).
    const CACHE_TTL: Duration = Duration::from_millis(500);

    fn backend(msg: impl Into<String>) -> MixerError {
        MixerError::Backend(msg.into())
    }

    /// ALSA simple-mixer volume (`snd_mixer_selem_*` playback volume).
    ///
    /// The mixer handle is opened per operation (volume changes are rare) so the
    /// type stays trivially `Send + Sync` and survives device hot-plug.
    pub struct AlsaMixer {
        device: String,
        controls: Vec<String>,
        index: u32,
        cache: Mutex<Option<(Instant, u8)>>,
    }

    impl AlsaMixer {
        pub fn new(device: &str, control: Option<&str>, index: u32) -> Result<Self, MixerError> {
            let controls = control_candidates(control);
            if device.contains('\0') || controls.iter().any(|c| c.contains('\0')) {
                return Err(backend("device/control name contains a NUL byte"));
            }
            Ok(Self {
                device: device.to_owned(),
                controls,
                index,
                cache: Mutex::new(None),
            })
        }

        fn with_selem<T>(
            &self,
            f: impl FnOnce(&Selem<'_>) -> Result<T, MixerError>,
        ) -> Result<T, MixerError> {
            let mixer = AlsaHandle::new(&self.device, false)
                .map_err(|e| backend(format!("cannot open `{}`: {e}", self.device)))?;
            for name in &self.controls {
                let id = SelemId::new(name, self.index);
                if let Some(selem) = mixer.find_selem(&id)
                    && selem.has_playback_volume()
                {
                    return f(&selem);
                }
            }
            Err(backend(format!(
                "no playback volume control {:?} (index {}) on `{}`",
                self.controls, self.index, self.device
            )))
        }
    }

    impl Mixer for AlsaMixer {
        fn name(&self) -> &str {
            "alsa"
        }

        fn set_volume(&self, v: u8) -> Result<(), MixerError> {
            let v = v.min(100);
            let applied = self.with_selem(|selem| {
                let (min, max) = selem.get_playback_volume_range();
                let raw = percent_to_raw(v, min, max);
                selem
                    .set_playback_volume_all(raw)
                    .map_err(|e| backend(format!("set volume failed: {e}")))?;
                Ok(raw_to_percent(raw, min, max))
            })?;
            *self.cache.lock() = Some((Instant::now(), applied));
            Ok(())
        }

        fn volume(&self) -> Result<u8, MixerError> {
            if let Some((at, v)) = *self.cache.lock()
                && at.elapsed() < CACHE_TTL
            {
                return Ok(v);
            }
            let v = self.with_selem(|selem| {
                let (min, max) = selem.get_playback_volume_range();
                let raw = selem
                    .get_playback_volume(SelemChannelId::FrontLeft)
                    .or_else(|_| selem.get_playback_volume(SelemChannelId::FrontRight))
                    .map_err(|e| backend(format!("get volume failed: {e}")))?;
                Ok(raw_to_percent(raw, min, max))
            })?;
            *self.cache.lock() = Some((Instant::now(), v));
            Ok(v)
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn out(output_type: &str, settings: &[(&str, &str)]) -> OutputConfig {
        let mut cfg = OutputConfig::cpal_default();
        cfg.output_type = output_type.to_owned();
        for (k, v) in settings {
            cfg.settings
                .insert((*k).to_owned(), toml::Value::String((*v).to_owned()));
        }
        cfg
    }

    /// Test double for a device mixer.
    struct FakeHardware(AtomicU8);

    impl Mixer for FakeHardware {
        fn name(&self) -> &str {
            "fake-hw"
        }
        fn set_volume(&self, v: u8) -> Result<(), MixerError> {
            self.0.store(v.min(100), Ordering::Release);
            Ok(())
        }
        fn volume(&self) -> Result<u8, MixerError> {
            Ok(self.0.load(Ordering::Acquire))
        }
    }

    #[test]
    fn mixer_type_parse() {
        assert_eq!(MixerType::parse("software"), Ok(MixerType::Software));
        assert_eq!(MixerType::parse(" Hardware "), Ok(MixerType::Hardware));
        assert_eq!(MixerType::parse("none"), Ok(MixerType::None));
        assert_eq!(MixerType::parse("NULL"), Ok(MixerType::None));
        assert_eq!(
            MixerType::parse("pulse"),
            Err(MixerError::UnknownType("pulse".into()))
        );
    }

    #[test]
    fn default_selection_is_software_for_every_output_type() {
        for ty in ["cpal", "default", "null", "fifo", "httpd", "pipewire", "x"] {
            assert_eq!(select_plugin(None, ty).unwrap().name, "software", "{ty}");
        }
    }

    #[test]
    fn explicit_software_and_none_selection() {
        assert_eq!(
            select_plugin(Some("software"), "cpal").unwrap().name,
            "software"
        );
        assert_eq!(select_plugin(Some("none"), "cpal").unwrap().name, "none");
        assert_eq!(select_plugin(Some("null"), "fifo").unwrap().name, "none");
    }

    #[test]
    fn unknown_mixer_type_is_an_error() {
        assert!(matches!(
            select_plugin(Some("bogus"), "cpal"),
            Err(MixerError::UnknownType(_))
        ));
    }

    #[test]
    fn hardware_rejected_for_outputs_without_native_mixer() {
        for ty in ["null", "fifo", "pipe", "httpd", "recorder", "pipewire"] {
            assert!(
                matches!(
                    select_plugin(Some("hardware"), ty),
                    Err(MixerError::NoHardwareMixer(_))
                ),
                "{ty}"
            );
        }
    }

    #[test]
    fn hardware_maps_to_alsa_when_compiled_in() {
        let alsa_built = cfg!(all(feature = "alsa-mixer", target_os = "linux"));
        for ty in ["cpal", "default", "alsa", "CPAL"] {
            match select_plugin(Some("hardware"), ty) {
                Ok(p) => {
                    assert!(alsa_built, "{ty}");
                    assert_eq!(p.name, "alsa");
                }
                Err(e) => {
                    assert!(!alsa_built, "{ty}");
                    assert!(matches!(e, MixerError::NoHardwareMixer(_)));
                }
            }
        }
    }

    #[test]
    fn registry_lookup_and_builtins() {
        assert!(plugin_by_name("software").is_some());
        assert!(plugin_by_name("none").is_some());
        assert!(plugin_by_name("pulse").is_none());
        assert_eq!(
            plugin_by_name("alsa").is_some(),
            cfg!(all(feature = "alsa-mixer", target_os = "linux"))
        );
    }

    #[test]
    fn percent_to_raw_maps_range_ends_and_middle() {
        assert_eq!(percent_to_raw(0, 0, 100), 0);
        assert_eq!(percent_to_raw(100, 0, 100), 100);
        assert_eq!(percent_to_raw(50, 0, 87), 44); // 43.5 rounds up
        assert_eq!(percent_to_raw(0, -10, 30), -10);
        assert_eq!(percent_to_raw(100, -10, 30), 30);
        assert_eq!(percent_to_raw(25, -10, 30), 0);
        // Above 100 is clamped; degenerate ranges collapse to min.
        assert_eq!(percent_to_raw(200, 0, 255), 255);
        assert_eq!(percent_to_raw(70, 5, 5), 5);
        assert_eq!(percent_to_raw(70, 9, 5), 9);
    }

    #[test]
    fn raw_to_percent_maps_range_ends_and_clamps() {
        assert_eq!(raw_to_percent(0, 0, 87), 0);
        assert_eq!(raw_to_percent(87, 0, 87), 100);
        assert_eq!(raw_to_percent(44, 0, 87), 51);
        assert_eq!(raw_to_percent(-50, 0, 87), 0);
        assert_eq!(raw_to_percent(500, 0, 87), 100);
        assert_eq!(raw_to_percent(0, -10, 30), 25);
        assert_eq!(raw_to_percent(3, 3, 3), 0);
    }

    #[test]
    fn percent_raw_roundtrip_is_exact_for_fine_ranges() {
        for (min, max) in [(0, 100), (0, 255), (0, 65535), (-1000, 0)] {
            for p in 0..=100u8 {
                let raw = percent_to_raw(p, min, max);
                assert!((min..=max).contains(&raw));
                assert_eq!(raw_to_percent(raw, min, max), p, "range {min}..{max}");
            }
        }
    }

    #[test]
    fn percent_raw_roundtrip_is_monotonic_for_coarse_ranges() {
        let (min, max) = (0, 31);
        let mut last = 0;
        for p in 0..=100u8 {
            let back = raw_to_percent(percent_to_raw(p, min, max), min, max);
            assert!(back >= last);
            last = back;
        }
        assert_eq!(last, 100);
    }

    #[test]
    fn mixer_device_derivation() {
        assert_eq!(mixer_device_for_pcm("hw:1,0").as_deref(), Some("hw:1"));
        assert_eq!(mixer_device_for_pcm("hw:2").as_deref(), Some("hw:2"));
        assert_eq!(
            mixer_device_for_pcm("hw:CARD=1,DEV=0").as_deref(),
            Some("hw:CARD=1")
        );
        assert_eq!(
            mixer_device_for_pcm("plughw:CARD=PCH,DEV=0").as_deref(),
            Some("hw:CARD=PCH")
        );
        assert_eq!(mixer_device_for_pcm("default"), None);
        assert_eq!(mixer_device_for_pcm("pipewire"), None);
        assert_eq!(mixer_device_for_pcm("hw:"), None);
        assert_eq!(mixer_device_for_pcm("sysdefault:CARD=PCH"), None);
    }

    #[test]
    fn control_candidate_order() {
        assert_eq!(control_candidates(None), vec!["PCM", "Master"]);
        assert_eq!(control_candidates(Some("  ")), vec!["PCM", "Master"]);
        assert_eq!(control_candidates(Some("Digital")), vec!["Digital"]);
    }

    #[test]
    fn params_defaults_and_overrides() {
        let sw = Arc::new(AtomicU8::new(100));
        let p = params_for_output(&out("null", &[]), &sw).unwrap();
        assert_eq!(p.device, "default");
        assert_eq!(p.control, None);
        assert_eq!(p.index, 0);

        let cfg = out(
            "cpal",
            &[
                ("mixer_device", "hw:3"),
                ("mixer_control", "Digital"),
                ("mixer_index", "2"),
            ],
        );
        let p = params_for_output(&cfg, &sw).unwrap();
        assert_eq!(p.device, "hw:3");
        assert_eq!(p.control.as_deref(), Some("Digital"));
        assert_eq!(p.index, 2);

        // The output's own hw: PCM supplies the mixer card for ALSA outputs.
        let cfg = out("cpal", &[("device", "hw:CARD=DAC,DEV=0")]);
        assert_eq!(params_for_output(&cfg, &sw).unwrap().device, "hw:CARD=DAC");

        assert_eq!(
            params_for_output(&out("cpal", &[("mixer_index", "x")]), &sw).unwrap_err(),
            MixerError::InvalidIndex("x".into())
        );
    }

    #[test]
    fn default_outputs_use_shared_software_mixer() {
        let sw = Arc::new(AtomicU8::new(100));
        let set = MixerSet::from_outputs(&[OutputConfig::cpal_default()], &sw);
        assert!(set.has_software());
        assert!(!set.has_hardware());
        assert!(set.controls_volume());
        set.set_volume(42).unwrap();
        assert_eq!(sw.load(Ordering::Acquire), 42);
        assert_eq!(set.volume(), Some(42));
        assert_eq!(set.hardware_volume(), None);
    }

    #[test]
    fn empty_output_list_still_has_software_mixer() {
        let sw = Arc::new(AtomicU8::new(100));
        let set = MixerSet::from_outputs(&[], &sw);
        assert!(set.has_software());
        set.set_volume(10).unwrap();
        assert_eq!(sw.load(Ordering::Acquire), 10);
    }

    #[test]
    fn none_mixer_rejects_volume() {
        let sw = Arc::new(AtomicU8::new(100));
        let set = MixerSet::from_outputs(&[out("cpal", &[("mixer_type", "none")])], &sw);
        assert!(!set.controls_volume());
        assert!(!set.has_software());
        assert_eq!(set.set_volume(10), Err(MixerError::NoMixer));
        assert_eq!(set.volume(), None);
        assert_eq!(sw.load(Ordering::Acquire), 100);
    }

    #[test]
    fn invalid_config_falls_back_to_software() {
        let sw = Arc::new(AtomicU8::new(100));
        let set = MixerSet::from_outputs(&[out("cpal", &[("mixer_type", "bogus")])], &sw);
        assert!(set.has_software());
        set.set_volume(30).unwrap();
        assert_eq!(sw.load(Ordering::Acquire), 30);
    }

    #[test]
    fn hardware_only_set_leaves_software_gain_at_unity() {
        let sw = Arc::new(AtomicU8::new(100));
        let hw = Arc::new(FakeHardware(AtomicU8::new(0)));
        let set = MixerSet::from_entries(vec![hw.clone() as Arc<dyn Mixer>]);
        assert!(!set.has_software());
        assert!(set.has_hardware());
        set.set_volume(35).unwrap();
        assert_eq!(hw.volume().unwrap(), 35);
        assert_eq!(sw.load(Ordering::Acquire), 100);
        assert_eq!(set.volume(), Some(35));
        assert_eq!(set.hardware_volume(), Some(35));
    }

    #[test]
    fn mixed_set_averages_volumes() {
        let sw = Arc::new(AtomicU8::new(100));
        let hw = Arc::new(FakeHardware(AtomicU8::new(0)));
        let none = Arc::new(NullMixer);
        let set = MixerSet::from_entries(vec![
            Arc::new(SoftwareMixer::new(sw.clone())) as Arc<dyn Mixer>,
            hw.clone() as Arc<dyn Mixer>,
            none as Arc<dyn Mixer>,
        ]);
        set.set_volume(80).unwrap();
        assert_eq!(set.volume(), Some(80));
        hw.0.store(60, Ordering::Release);
        assert_eq!(set.volume(), Some(70)); // (80 + 60) / 2; none ignored
    }
}
