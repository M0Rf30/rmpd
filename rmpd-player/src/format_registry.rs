//! Single source of truth for "which formats can rmpd scan/play", derived at runtime from
//! Symphonia's own probe and codec registries instead of hand-maintained extension/mime lists.
//!
//! Every consumer that used to keep its own copy of the supported-extension list (the library
//! scanner, the decoder's suffix table, and the `decoders` MPD command reply) should read from
//! here so the three can never drift apart again.
//!
//! Lives in `rmpd-player` because it already depends on `symphonia`, and both `rmpd-library` and
//! `rmpd-protocol` already depend on `rmpd-player`, making this the least invasive shared home.

use std::sync::LazyLock;

/// A container/demuxer ("format") reader registered with Symphonia's probe, along with the file
/// extensions and MIME types the probe will match against it.
///
/// `plugin` mirrors MPD's `decoders` command terminology: one entry per registered reader
/// implementation (Symphonia's `FormatInfo::short_name`), not per audio codec.
#[derive(Debug, Clone)]
pub struct RegisteredFormat {
    pub plugin: &'static str,
    pub extensions: Vec<&'static str>,
    pub mime_types: Vec<&'static str>,
}

/// Extensions rmpd should treat as scannable that Symphonia's probe does not declare against any
/// registered reader, paired with the reader that actually demuxes them in practice. Keep this
/// list as small as possible; anything Symphonia already declares must NOT be repeated here.
///
/// - "alac": ALAC audio is only ever carried inside an ISO-BMFF container (.m4a/.mov), which the
///   isomp4 reader already covers by extension. A bare `*.alac` file (produced by some rippers)
///   still probes successfully via isomp4's marker-based detection, so it is scannable even
///   though isomp4 does not list "alac" as one of its own extensions.
/// - "mka": Matroska Audio is an audio-only variant of the same container the "matroska" reader
///   (Symphonia's Matroska/WebM demuxer, format short name "matroska") already demuxes; only
///   "webm"/"mkv" are declared as extensions upstream.
const EXTRA_EXTENSION_ALIASES: &[(&str, &str)] = &[("alac", "isomp4"), ("mka", "matroska")];

/// Every format reader Symphonia's default probe has registered, grouped by plugin (reader)
/// name, sorted for deterministic output.
#[allow(
    clippy::disallowed_types,
    reason = "ordering is required: `decoders` output must be deterministic"
)]
pub static FORMAT_PLUGINS: LazyLock<Vec<RegisteredFormat>> = LazyLock::new(|| {
    use std::collections::BTreeMap;
    use symphonia::core::formats::probe::RegisteredFormatInfo;

    let probe = symphonia::default::get_probe();
    let mut by_plugin: BTreeMap<&'static str, (Vec<&'static str>, Vec<&'static str>)> =
        BTreeMap::new();

    for reg in probe.formats() {
        // Metadata-only readers (ID3, APEv2 tags, etc.) aren't playable containers; only
        // container/demuxer registrations should show up as "decoders".
        let RegisteredFormatInfo::Format(info) = reg.info() else {
            continue;
        };

        let entry = by_plugin.entry(info.short_name).or_default();
        for ext in reg.extensions() {
            if !entry.0.contains(ext) {
                entry.0.push(ext);
            }
        }
        for mime in reg.mime_types() {
            if !entry.1.contains(mime) {
                entry.1.push(mime);
            }
        }
    }

    for (ext, plugin) in EXTRA_EXTENSION_ALIASES {
        let entry = by_plugin.entry(plugin).or_default();
        if !entry.0.contains(ext) {
            entry.0.push(ext);
        }
    }

    by_plugin
        .into_iter()
        .map(|(plugin, (extensions, mime_types))| RegisteredFormat {
            plugin,
            extensions,
            mime_types,
        })
        .collect()
});

/// Every file extension (lowercase, no leading dot) any registered format reader will match.
pub static SUPPORTED_EXTENSIONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut exts: Vec<&'static str> = FORMAT_PLUGINS
        .iter()
        .flat_map(|f| f.extensions.iter().copied())
        .collect();
    exts.sort_unstable();
    exts.dedup();
    exts
});

/// Every MIME type any registered format reader declares.
pub static SUPPORTED_MIME_TYPES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut mimes: Vec<&'static str> = FORMAT_PLUGINS
        .iter()
        .flat_map(|f| f.mime_types.iter().copied())
        .collect();
    mimes.sort_unstable();
    mimes.dedup();
    mimes
});

/// Whether `ext` (without a leading dot, any case) names a format rmpd can scan/probe. The
/// single source of truth for every extension filter in rmpd.
#[must_use]
pub fn is_supported_extension(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.iter().any(|e| *e == ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_expected_extensions() {
        for ext in [
            "flac", "mp3", "ogg", "oga", "wav", "wave", "aiff", "aif", "m4a", "mov", "caf", "ape",
            "wv", "dsf", "dff", "mka", "webm",
        ] {
            assert!(
                is_supported_extension(ext),
                "expected {ext} to be supported"
            );
            assert!(
                is_supported_extension(&ext.to_uppercase()),
                "case-insensitive: {ext}"
            );
        }
    }

    #[test]
    fn caf_is_a_registered_plugin() {
        assert!(FORMAT_PLUGINS.iter().any(|f| f.plugin == "caf"));
    }

    #[test]
    fn alac_alias_resolves_to_isomp4() {
        let isomp4 = FORMAT_PLUGINS
            .iter()
            .find(|f| f.plugin == "isomp4")
            .expect("isomp4 registered");
        assert!(isomp4.extensions.contains(&"alac"));
    }
}
