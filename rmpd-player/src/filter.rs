// SPDX-License-Identifier: MIT OR Apache-2.0

//! DSP filter registry (named, configured filter instances) and per-output software mixer seam.
//!
//! The DSP stage trait [`AudioFilter`], the [`FilterChain`] and the [`playcore::filter::VolumeFilter`] live in
//! [`playcore::filter`].
//!
//! [`Mixer`] is the per-output volume seam; the registry of mixer plugins
//! (software, hardware/ALSA, none) lives in [`crate::mixer`].
//! [`SoftwareMixer`] is the software implementation backed by the same atomic.

use crate::dsp;
use crate::mixer::MixerError;
use parking_lot::RwLock;
use playcore::filter::{AudioFilter, FilterChain};
use rmpd_core::config::{FilterConfig, OutputConfig, unknown_setting_messages};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use thiserror::Error;
use tracing::warn;

// ── Filter plugin registry ───────────────────────────────────────────────────
//
// Mirrors MPD's filter plugins conceptually: `[[filter]]` blocks (`name`,
// `type`, settings) are named instances of a compile-time plugin; an output's
// `filters = ["a", "b"]` (or the global `[audio].filters`) selects the chain.

/// Errors raised while building a configured filter.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FilterError {
    #[error("unknown filter type `{0}`")]
    UnknownType(String),
    #[error("invalid filter settings: {0}")]
    Config(String),
}

/// Inputs handed to a [`FilterFactory`].
#[derive(Debug, Clone, Copy)]
pub struct FilterParams<'a> {
    /// Sample rate of the stream the filter will process.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u8,
    /// The `[[filter]]` block's flattened settings (without name/type/enabled).
    pub settings: &'a toml::Table,
}

/// Builds one filter instance. Synchronous and free of I/O.
pub type FilterFactory = fn(&FilterParams<'_>) -> Result<Box<dyn AudioFilter>, FilterError>;

/// One filter plugin: registry name, accepted setting keys, factory.
pub struct FilterPlugin {
    pub name: &'static str,
    pub settings: &'static [&'static str],
    pub factory: FilterFactory,
}

/// Compile-time filter registry, selected by `[[filter]].type`.
pub static FILTER_PLUGINS: &[FilterPlugin] = &[
    FilterPlugin {
        name: "normalize",
        settings: dsp::NORMALIZE_SETTINGS,
        factory: dsp::normalize_factory,
    },
    FilterPlugin {
        name: "equalizer",
        settings: dsp::EQUALIZER_SETTINGS,
        factory: dsp::equalizer_factory,
    },
    FilterPlugin {
        name: "route",
        settings: dsp::ROUTE_SETTINGS,
        factory: dsp::route_factory,
    },
    FilterPlugin {
        name: "channels",
        settings: dsp::ROUTE_SETTINGS,
        factory: dsp::route_factory,
    },
];

/// Look a filter plugin up by registry name (case-insensitive).
pub fn filter_plugin_by_name(name: &str) -> Option<&'static FilterPlugin> {
    FILTER_PLUGINS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// Build one configured filter for the given stream format.
pub fn build_filter(
    cfg: &FilterConfig,
    sample_rate: u32,
    channels: u8,
) -> Result<Box<dyn AudioFilter>, FilterError> {
    let plugin = filter_plugin_by_name(&cfg.filter_type)
        .ok_or_else(|| FilterError::UnknownType(cfg.filter_type.clone()))?;
    (plugin.factory)(&FilterParams {
        sample_rate,
        channels,
        settings: &cfg.settings,
    })
}

/// The configured `[[filter]]` definitions plus the global chain, resolved per
/// output into a [`FilterChain`].
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    defs: Vec<FilterConfig>,
    global: Vec<String>,
}

impl FilterSet {
    /// `defs`: all `[[filter]]` blocks; `global`: `[audio].filters`.
    #[must_use]
    pub fn new(defs: &[FilterConfig], global: &[String]) -> Self {
        Self {
            defs: defs.to_vec(),
            global: global.to_vec(),
        }
    }

    /// Filter names selected for `output`: its own `filters` setting (array of
    /// names, or one comma-separated string) when present — an empty list
    /// disables filtering for that output — otherwise the global chain.
    #[must_use]
    pub fn chain_names(&self, output: &OutputConfig) -> Vec<String> {
        match output.settings.get("filters") {
            Some(toml::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            Some(toml::Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => self.global.clone(),
        }
    }

    fn find(&self, name: &str) -> Option<&FilterConfig> {
        self.defs.iter().find(|d| d.name == name)
    }

    /// Resolved, enabled definitions of `output`'s chain (unknown names are
    /// skipped; [`Self::diagnostics`] reports them).
    fn resolve<'a>(&'a self, output: &OutputConfig) -> Vec<&'a FilterConfig> {
        self.chain_names(output)
            .iter()
            .filter_map(|n| self.find(n))
            .filter(|d| d.enabled)
            .collect()
    }

    /// Stable description of `output`'s resolved chain: empty when no filter
    /// applies. Part of the output-reuse key, so changing the filter setup
    /// rebuilds the outputs.
    #[must_use]
    pub fn fingerprint(&self, output: &OutputConfig) -> String {
        self.resolve(output)
            .iter()
            .map(|d| format!("{}:{}:{:?}", d.name, d.filter_type, d.settings))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Build the chain of `output` for the stream format. A filter that fails
    /// to build is skipped with a warning (never fatal).
    #[must_use]
    pub fn build_chain(
        &self,
        output: &OutputConfig,
        sample_rate: u32,
        channels: u8,
    ) -> FilterChain {
        let mut chain = FilterChain::new();
        for def in self.resolve(output) {
            match build_filter(def, sample_rate, channels) {
                Ok(f) => chain.push(f),
                Err(e) => warn!("filter `{}` skipped: {e}", def.name),
            }
        }
        chain.configure(sample_rate, channels);
        chain
    }

    /// Configuration problems, as human-readable warnings (values are never
    /// included): duplicate names, unknown types / setting keys, settings the
    /// factory rejects, and references to undefined filters.
    #[must_use]
    pub fn diagnostics(&self, outputs: &[OutputConfig]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, def) in self.defs.iter().enumerate() {
            if self.defs[..i].iter().any(|d| d.name == def.name) {
                out.push(format!(
                    "duplicate [[filter]] name `{}`; the first block wins",
                    def.name
                ));
            }
            let Some(plugin) = filter_plugin_by_name(&def.filter_type) else {
                out.push(format!(
                    "unknown filter type `{}` in [[filter]] `{}`",
                    def.filter_type, def.name
                ));
                continue;
            };
            out.extend(unknown_setting_messages(
                "filter",
                &def.name,
                &def.settings,
                plugin.settings,
            ));
            if let Err(e) = build_filter(def, 44_100, 2) {
                out.push(format!("[[filter]] `{}`: {e}", def.name));
            }
        }
        for name in &self.global {
            if self.find(name).is_none() {
                out.push(format!(
                    "[audio].filters references undefined filter `{name}`"
                ));
            }
        }
        for output in outputs {
            if output.settings.contains_key("filters") {
                for name in self.chain_names(output) {
                    if self.find(&name).is_none() {
                        out.push(format!(
                            "[[output]] `{}` references undefined filter `{name}`",
                            output.name
                        ));
                    }
                }
            }
        }
        out
    }
}

static ACTIVE_FILTERS: RwLock<Option<Arc<FilterSet>>> = RwLock::new(None);

/// Install the process-wide filter configuration (like
/// `rmpd_stream::configure`). Takes effect the next time an output is opened.
/// Returns configuration warnings for the caller to log.
pub fn configure(
    defs: &[FilterConfig],
    global: &[String],
    outputs: &[OutputConfig],
) -> Vec<String> {
    let set = FilterSet::new(defs, global);
    let diagnostics = set.diagnostics(outputs);
    *ACTIVE_FILTERS.write() = Some(Arc::new(set));
    diagnostics
}

/// Chain of `output` for the active configuration; empty (a no-op) when no
/// filters are configured.
#[must_use]
pub fn chain_for_output(output: &OutputConfig, sample_rate: u32, channels: u8) -> FilterChain {
    let active = ACTIVE_FILTERS.read().clone();
    match active {
        Some(set) => set.build_chain(output, sample_rate, channels),
        None => FilterChain::new(),
    }
}

/// Fingerprint of `output`'s chain in the active configuration (empty when
/// none), for the output-reuse key.
#[must_use]
pub fn chain_fingerprint(output: &OutputConfig) -> String {
    let active = ACTIVE_FILTERS.read().clone();
    match active {
        Some(set) => set.fingerprint(output),
        None => String::new(),
    }
}

// ── Mixer trait & SoftwareMixer ──────────────────────────────────────────────

/// Per-output volume control seam.
///
/// Implementations: [`SoftwareMixer`] (in-process gain), the ALSA hardware
/// mixer and the null mixer (see [`crate::mixer`]).  Calls are synchronous and
/// may block briefly (hardware mixers talk to the sound card), so async
/// callers should keep them off the hot path.
pub trait Mixer: Send + Sync {
    /// Registry name (`"software"`, `"alsa"`, `"none"`).
    fn name(&self) -> &str;

    /// `true` when volume is applied by rmpd's own gain stage (the
    /// [`playcore::filter::VolumeFilter`] / `OutputControl` gain) rather than by a device.
    fn is_software(&self) -> bool {
        false
    }

    /// `false` for the null mixer: volume cannot be set or read at all.
    fn controls_volume(&self) -> bool {
        true
    }

    /// Set the volume in percent (values above 100 are clamped).
    fn set_volume(&self, v: u8) -> Result<(), MixerError>;

    /// Current volume in percent.
    fn volume(&self) -> Result<u8, MixerError>;
}

/// Software [`Mixer`] backed by an `Arc<AtomicU8>`.
///
/// The atomic can be shared with a [`playcore::filter::VolumeFilter`] so that a volume change
/// through the mixer is immediately visible in the filter without any
/// additional coordination.
pub struct SoftwareMixer {
    volume: Arc<AtomicU8>,
}

impl SoftwareMixer {
    pub fn new(volume: Arc<AtomicU8>) -> Self {
        Self { volume }
    }

    /// Clone the underlying handle so it can be passed to a [`playcore::filter::VolumeFilter`].
    pub fn volume_handle(&self) -> Arc<AtomicU8> {
        self.volume.clone()
    }
}

impl Mixer for SoftwareMixer {
    fn name(&self) -> &str {
        "software"
    }

    fn is_software(&self) -> bool {
        true
    }

    fn set_volume(&self, v: u8) -> Result<(), MixerError> {
        self.volume.store(v.min(100), Ordering::Release);
        Ok(())
    }

    fn volume(&self) -> Result<u8, MixerError> {
        Ok(self.volume.load(Ordering::Acquire))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU8;

    // SoftwareMixer: set/get roundtrip.
    #[test]
    fn software_mixer_set_get() {
        let vol = Arc::new(AtomicU8::new(0));
        let mixer = SoftwareMixer::new(Arc::clone(&vol));
        mixer.set_volume(75).unwrap();
        assert_eq!(mixer.volume().unwrap(), 75);
    }

    // SoftwareMixer: values >100 are clamped to 100.
    #[test]
    fn software_mixer_clamps_above_100() {
        let vol = Arc::new(AtomicU8::new(0));
        let mixer = SoftwareMixer::new(Arc::clone(&vol));
        mixer.set_volume(200).unwrap();
        assert_eq!(mixer.volume().unwrap(), 100);
        mixer.set_volume(101).unwrap();
        assert_eq!(mixer.volume().unwrap(), 100);
    }

    // SoftwareMixer: volume_handle() shares the same atomic.
    #[test]
    fn software_mixer_volume_handle_shares_atomic() {
        let vol = Arc::new(AtomicU8::new(50));
        let mixer = SoftwareMixer::new(Arc::clone(&vol));
        let handle = mixer.volume_handle();
        mixer.set_volume(80).unwrap();
        // handle sees the update immediately
        assert_eq!(handle.load(Ordering::Acquire), 80);
    }

    // ── Registry / FilterSet ────────────────────────────────────────────────

    fn filter_cfg(name: &str, ty: &str, settings: &str) -> FilterConfig {
        FilterConfig {
            name: name.to_owned(),
            filter_type: ty.to_owned(),
            enabled: true,
            settings: settings.parse::<toml::Table>().unwrap(),
        }
    }

    fn output_cfg(name: &str, settings: &str) -> OutputConfig {
        OutputConfig {
            name: name.to_owned(),
            output_type: "null".to_owned(),
            enabled: true,
            settings: settings.parse::<toml::Table>().unwrap(),
        }
    }

    #[test]
    fn registry_names_and_lookup() {
        let names: Vec<&str> = FILTER_PLUGINS.iter().map(|p| p.name).collect();
        assert_eq!(names, ["normalize", "equalizer", "route", "channels"]);
        assert!(filter_plugin_by_name("Equalizer").is_some());
        assert!(filter_plugin_by_name("reverb").is_none());
    }

    #[test]
    fn build_filter_rejects_unknown_type() {
        let err = build_filter(&filter_cfg("x", "reverb", ""), 44_100, 2)
            .err()
            .unwrap();
        assert_eq!(err, FilterError::UnknownType("reverb".into()));
    }

    #[test]
    fn empty_set_gives_empty_chain_and_fingerprint() {
        let set = FilterSet::default();
        let out = output_cfg("o", "");
        assert!(set.build_chain(&out, 44_100, 2).is_empty());
        assert!(set.fingerprint(&out).is_empty());
    }

    #[test]
    fn global_chain_and_per_output_override() {
        let defs = [
            filter_cfg("mono", "route", "mode = \"mono\""),
            filter_cfg("swap", "channels", "mode = \"swap\""),
        ];
        let set = FilterSet::new(&defs, &["mono".to_owned()]);

        let global = output_cfg("a", "");
        assert_eq!(set.chain_names(&global), ["mono"]);
        assert_eq!(set.build_chain(&global, 44_100, 2).len(), 1);

        let own = output_cfg("b", "filters = [\"swap\", \"mono\"]");
        assert_eq!(set.chain_names(&own), ["swap", "mono"]);
        assert_eq!(set.build_chain(&own, 44_100, 2).len(), 2);

        let none = output_cfg("c", "filters = []");
        assert!(set.chain_names(&none).is_empty());
        assert!(set.build_chain(&none, 44_100, 2).is_empty());
        assert!(set.fingerprint(&none).is_empty());

        let csv = output_cfg("d", "filters = \"swap, mono\"");
        assert_eq!(set.chain_names(&csv), ["swap", "mono"]);

        assert_ne!(set.fingerprint(&global), set.fingerprint(&own));
    }

    #[test]
    fn chain_runs_filters_in_order() {
        let defs = [
            filter_cfg("swap", "route", "mode = \"swap\""),
            filter_cfg("left", "route", "mode = \"left\""),
        ];
        let set = FilterSet::new(&defs, &["swap".to_owned(), "left".to_owned()]);
        let mut chain = set.build_chain(&output_cfg("o", ""), 44_100, 2);
        assert_eq!(chain.names(), ["route", "route"]);
        // swap: (1,2)->(2,1); then left copies channel 0: (2,2).
        let mut buf = [1.0, 2.0];
        chain.apply(&mut buf);
        assert_eq!(buf, [2.0, 2.0]);
    }

    #[test]
    fn disabled_and_broken_filters_are_skipped() {
        let mut off = filter_cfg("off", "route", "");
        off.enabled = false;
        let defs = [off, filter_cfg("bad", "equalizer", "bands = 3")];
        let set = FilterSet::new(&defs, &["off".to_owned(), "bad".to_owned()]);
        assert!(set.build_chain(&output_cfg("o", ""), 44_100, 2).is_empty());
    }

    #[test]
    fn diagnostics_report_problems_without_values() {
        let defs = [
            filter_cfg("a", "route", "mode = \"swap\""),
            filter_cfg("a", "route", "mode = \"swap\""),
            filter_cfg("b", "reverb", ""),
            filter_cfg("c", "normalize", "targt_db = -9"),
            filter_cfg("d", "equalizer", "bands = 3"),
        ];
        let set = FilterSet::new(&defs, &["ghost".to_owned()]);
        let outs = [output_cfg("o", "filters = [\"missing\"]")];
        let msgs = set.diagnostics(&outs);
        let all = msgs.join("\n");
        assert!(all.contains("duplicate [[filter]] name `a`"), "{all}");
        assert!(all.contains("unknown filter type `reverb`"), "{all}");
        assert!(all.contains("unknown setting `targt_db`"), "{all}");
        assert!(all.contains("[[filter]] `d`"), "{all}");
        assert!(all.contains("undefined filter `ghost`"), "{all}");
        assert!(all.contains("undefined filter `missing`"), "{all}");
        assert!(!all.contains("-9"), "values must not be echoed: {all}");

        let clean = FilterSet::new(&[filter_cfg("a", "route", "")], &["a".to_owned()]);
        assert!(clean.diagnostics(&[output_cfg("o", "")]).is_empty());
    }
}
