// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cover-art providers: the compile-time [`ARTWORK_PLUGINS`] registry and the
//! helpers that turn `[[artwork]]` blocks into an
//! [`rmpd_plugin::ArtworkResolver`].
//!
//! To add one: create a module implementing [`rmpd_plugin::ArtworkProvider`],
//! expose a `SETTINGS` const, a factory fn and a `PLUGIN` const, and list it
//! in [`ARTWORK_PLUGINS`] behind its Cargo feature.

use rmpd_core::config::{ArtworkConfig, unknown_setting_messages};
use rmpd_plugin::{ArtworkPlugin, ArtworkProvider, ArtworkResolver, PluginError};
use std::sync::Arc;

#[cfg(feature = "coverart")]
pub mod coverartarchive;

/// All compiled-in artwork providers.
pub static ARTWORK_PLUGINS: &[ArtworkPlugin] = &[
    #[cfg(feature = "coverart")]
    coverartarchive::PLUGIN,
];

/// Look up a provider type by (case-insensitive) name.
#[must_use]
pub fn find_artwork_plugin(artwork_type: &str) -> Option<&'static ArtworkPlugin> {
    let ty = artwork_type.to_lowercase();
    ARTWORK_PLUGINS.iter().find(|p| p.name == ty)
}

/// Construct a provider from its `[[artwork]]` block, warning about unknown
/// setting keys. Unknown types yield [`PluginError::Config`].
pub fn create_provider(cfg: &ArtworkConfig) -> Result<Box<dyn ArtworkProvider>, PluginError> {
    let plugin = find_artwork_plugin(&cfg.artwork_type).ok_or_else(|| {
        PluginError::Config(format!(
            "unknown artwork provider type: {} (not compiled in?)",
            cfg.artwork_type.to_lowercase()
        ))
    })?;
    for msg in unknown_setting_messages("artwork", &cfg.name, &cfg.settings, plugin.settings) {
        tracing::warn!("{msg}");
    }
    (plugin.factory)(cfg)
}

/// Build the provider chain from every enabled `[[artwork]]` block, in config
/// order. Bad blocks are logged and skipped.
#[must_use]
pub fn build_resolver(cfgs: &[ArtworkConfig]) -> ArtworkResolver {
    let mut providers: Vec<Arc<dyn ArtworkProvider>> = Vec::new();
    for cfg in cfgs.iter().filter(|c| c.enabled) {
        match create_provider(cfg) {
            Ok(p) => {
                tracing::info!(
                    name = %cfg.name,
                    artwork_type = %cfg.artwork_type,
                    "artwork provider enabled"
                );
                providers.push(Arc::from(p));
            }
            Err(e) => tracing::warn!(
                name = %cfg.name,
                artwork_type = %cfg.artwork_type,
                "failed to initialise artwork provider, skipping: {e}"
            ),
        }
    }
    ArtworkResolver::new(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(ty: &str, enabled: bool) -> ArtworkConfig {
        ArtworkConfig {
            name: "art".to_owned(),
            artwork_type: ty.to_owned(),
            enabled,
            settings: toml::Table::new(),
        }
    }

    #[test]
    fn unknown_type_is_config_error() {
        let err = create_provider(&block("bogus", true)).err();
        assert!(matches!(err, Some(PluginError::Config(_))));
        assert!(find_artwork_plugin("bogus").is_none());
    }

    #[test]
    fn bad_and_disabled_blocks_are_skipped() {
        let r = build_resolver(&[block("bogus", true), block("coverartarchive", false)]);
        assert!(r.is_empty());
        assert!(build_resolver(&[]).is_empty());
    }

    #[cfg(feature = "coverart")]
    #[test]
    fn coverartarchive_is_registered_case_insensitively() {
        assert!(find_artwork_plugin("CoverArtArchive").is_some());
        let r = build_resolver(&[block("coverartarchive", true)]);
        assert!(!r.is_empty());
    }
}
