// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::cargo_common_metadata)]

//! rmpd-integrations — compile-time registry of background integrations
//! (scrobblers, notifiers, ...). Each integration implements
//! [`rmpd_plugin::Integration`], is listed in [`INTEGRATION_PLUGINS`] behind
//! its own Cargo feature, and is configured by an `[[integration]]` block.
//!
//! To add one: create a module, implement `Integration`, expose a
//! `SETTINGS` const and a factory fn, and add an `IntegrationPlugin` entry to
//! [`INTEGRATION_PLUGINS`] (feature-gated with `#[cfg(feature = "...")]`).

use rmpd_core::config::{IntegrationConfig, unknown_setting_messages};
use rmpd_core::event::EventBus;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{
    Integration, IntegrationContext, IntegrationPlugin, PlayerHandle, ShutdownSignal,
};
use std::path::Path;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub mod artwork;
pub mod builtin;
#[cfg(feature = "http-api")]
pub mod http_api;
#[cfg(feature = "lastfm")]
pub mod lastfm;
#[cfg(feature = "listenbrainz")]
pub mod listenbrainz;
#[cfg(feature = "mdns")]
pub mod mdns;
#[cfg(all(feature = "mpris", not(target_os = "macos")))]
pub mod mpris;
#[cfg(any(feature = "listenbrainz", feature = "lastfm"))]
pub mod scrobble;
#[cfg(feature = "webhook")]
pub mod webhook;

pub use artwork::{ARTWORK_PLUGINS, build_resolver as build_artwork_resolver};
pub use builtin::{mdns_config, synthesize_builtin};

/// All compiled-in integrations.
pub static INTEGRATION_PLUGINS: &[IntegrationPlugin] = &[
    #[cfg(all(feature = "mpris", not(target_os = "macos")))]
    mpris::PLUGIN,
    #[cfg(feature = "mdns")]
    mdns::PLUGIN,
    #[cfg(feature = "listenbrainz")]
    listenbrainz::PLUGIN,
    #[cfg(feature = "lastfm")]
    lastfm::PLUGIN,
    #[cfg(feature = "webhook")]
    webhook::PLUGIN,
    #[cfg(feature = "http-api")]
    http_api::PLUGIN,
];

/// Look up a plugin by (case-insensitive) type name.
#[must_use]
pub fn find_plugin(integration_type: &str) -> Option<&'static IntegrationPlugin> {
    let ty = integration_type.to_lowercase();
    INTEGRATION_PLUGINS.iter().find(|p| p.name == ty)
}

/// Construct an integration from its `[[integration]]` block, warning about
/// unknown setting keys. Unknown types yield [`PluginError::Config`].
pub fn create_integration(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    let plugin = find_plugin(&cfg.integration_type).ok_or_else(|| {
        PluginError::Config(format!(
            "unknown integration type: {}",
            cfg.integration_type.to_lowercase()
        ))
    })?;
    for msg in unknown_setting_messages("integration", &cfg.name, &cfg.settings, plugin.settings) {
        tracing::warn!("{msg}");
    }
    (plugin.factory)(cfg)
}

/// Build and spawn every enabled integration. Bad blocks are logged and
/// skipped. Each integration gets its own event subscription, a clone of
/// `player`, a per-instance directory under `state_dir`, and `shutdown`.
/// Returns the task handles so the caller can await a clean stop.
pub fn spawn_integrations(
    cfgs: &[IntegrationConfig],
    events: &EventBus,
    player: &Arc<dyn PlayerHandle>,
    state_dir: &Path,
    shutdown: &ShutdownSignal,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::new();
    for cfg in cfgs.iter().filter(|c| c.enabled) {
        let integration = match create_integration(cfg) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(
                    name = %cfg.name,
                    integration_type = %cfg.integration_type,
                    "failed to initialise integration, skipping: {e}"
                );
                continue;
            }
        };
        let dir = state_dir.join(&cfg.name);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(name = %cfg.name, "cannot create integration state dir: {e}");
            continue;
        }
        let ctx = IntegrationContext {
            events: events.subscribe(),
            player: Arc::clone(player),
            state_dir: dir,
            shutdown: shutdown.clone(),
        };
        let name = cfg.name.clone();
        tracing::info!(%name, "starting integration");
        handles.push(tokio::spawn(async move {
            if let Err(e) = integration.run(ctx).await {
                tracing::warn!(%name, "integration stopped with error: {e}");
            }
        }));
    }
    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_type_is_config_error() {
        let cfg = IntegrationConfig {
            name: "x".to_owned(),
            integration_type: "bogus".to_owned(),
            enabled: true,
            settings: toml::Table::new(),
        };
        assert!(matches!(
            create_integration(&cfg),
            Err(PluginError::Config(_))
        ));
    }
}
