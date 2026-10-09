// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Compile-time name→factory registry for music-source backends.
//!
//! Mirrors `OUTPUT_PLUGINS` in `rmpd-player/src/output_registry.rs` exactly:
//! a `const` slice of `(&str, SourceFactory)` pairs selected by lowercased
//! `source_type`. No I/O happens at selection time.

use crate::filesystem::filesystem_source_factory;
use rmpd_core::config::{SourceConfig, unknown_setting_messages};
use rmpd_plugin::source::{MusicSource, SourceError};

/// A sync, no-I/O factory: constructs a boxed source from config or returns a
/// `SourceError::Config` if the config is invalid.
pub type SourceFactory = fn(&SourceConfig) -> Result<Box<dyn MusicSource>, SourceError>;

/// One registry entry for a source backend.
#[derive(Clone, Copy)]
pub struct SourcePlugin {
    /// Matches `[[source]] type =` (lowercase).
    pub name: &'static str,
    /// Setting keys the backend accepts (besides `name`/`type`/`enabled`);
    /// any other key in the block produces a warning at startup.
    pub settings: &'static [&'static str],
    pub factory: SourceFactory,
}

/// All compiled-in source backends, in priority order.
pub static SOURCE_PLUGINS: &[SourcePlugin] = &[
    SourcePlugin {
        name: "filesystem",
        settings: crate::filesystem::SETTINGS,
        factory: filesystem_source_factory,
    },
    #[cfg(feature = "subsonic")]
    SourcePlugin {
        name: "subsonic",
        settings: crate::subsonic::SETTINGS,
        factory: crate::subsonic::subsonic_source_factory,
    },
];

/// Select and construct a `MusicSource` from a `[[source]]` config block.
///
/// Looks up `cfg.source_type` (lowercased) in `SOURCE_PLUGINS`, warns about
/// unknown setting keys, and calls the matching factory. Returns
/// `SourceError::Config` for unknown types.
pub fn create_source(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let ty = cfg.source_type.to_lowercase();
    let plugin = SOURCE_PLUGINS
        .iter()
        .find(|p| p.name == ty)
        .ok_or_else(|| SourceError::Config(format!("unknown source type: {ty}")))?;
    for msg in unknown_setting_messages("source", &cfg.name, &cfg.settings, plugin.settings) {
        tracing::warn!("{msg}");
    }
    (plugin.factory)(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::config::SourceConfig;
    use rmpd_plugin::source::SourceError;

    fn make_cfg(source_type: &str) -> SourceConfig {
        SourceConfig {
            name: "test".to_owned(),
            source_type: source_type.to_owned(),
            enabled: true,
            settings: {
                let mut t = toml::Table::new();
                t.insert(
                    "music_directory".to_owned(),
                    toml::Value::String("/tmp".to_owned()),
                );
                t.insert(
                    "db".to_owned(),
                    toml::Value::String("/tmp/test.db".to_owned()),
                );
                t
            },
        }
    }

    #[test]
    fn filesystem_type_ok() {
        let cfg = make_cfg("filesystem");
        let result = create_source(&cfg);
        assert!(result.is_ok(), "filesystem source should construct ok");
    }

    #[test]
    fn unknown_type_returns_config_error() {
        let cfg = make_cfg("bogus");
        let result = create_source(&cfg);
        match result {
            Err(SourceError::Config(msg)) => {
                assert!(msg.contains("bogus"), "error message should name the type");
            }
            Err(e) => panic!("expected SourceError::Config, got a different error: {e}"),
            Ok(_) => panic!("expected SourceError::Config, got Ok(source)"),
        }
    }
}
