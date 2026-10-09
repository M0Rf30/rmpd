// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Config synthesis for the built-in integrations (MPRIS, mDNS).
//!
//! Legacy `[network]` switches (`media_controls`/`mpris`, `zeroconf_enabled`,
//! `zeroconf_name`) keep working: at startup they are translated into
//! `[[integration]]` blocks, unless the user already configured an
//! integration of that type explicitly (so explicit blocks win and nothing is
//! started twice).

use rmpd_core::config::{IntegrationConfig, NetworkConfig};

fn has_type(existing: &[IntegrationConfig], ty: &str) -> bool {
    existing
        .iter()
        .any(|c| c.integration_type.eq_ignore_ascii_case(ty))
}

fn block(name: &str, ty: &str, settings: toml::Table) -> IntegrationConfig {
    IntegrationConfig {
        name: name.to_owned(),
        integration_type: ty.to_owned(),
        enabled: true,
        settings,
    }
}

/// Integration blocks implied by the legacy network switches that can be
/// started right away: currently MPRIS (Linux; `network.media_controls`).
///
/// macOS Now Playing is intentionally not handled here: it must run on the
/// process main thread (see `PLUGIN_ARCHITECTURE.md`).
#[must_use]
pub fn synthesize_builtin(
    network: &NetworkConfig,
    existing: &[IntegrationConfig],
) -> Vec<IntegrationConfig> {
    let mut out = Vec::new();
    if cfg!(not(target_os = "macos")) && network.media_controls && !has_type(existing, "mpris") {
        out.push(block("mpris", "mpris", toml::Table::new()));
    }
    out
}

/// The mDNS advertisement block implied by `network.zeroconf_enabled`, for the
/// port that was actually bound. Created after the listener is up because the
/// port may differ from the config (`--port`, socket activation). `None` when
/// zeroconf is disabled or an explicit `mdns` integration exists.
#[must_use]
pub fn mdns_config(
    network: &NetworkConfig,
    existing: &[IntegrationConfig],
    port: u16,
) -> Option<IntegrationConfig> {
    if !network.zeroconf_enabled || has_type(existing, "mdns") {
        return None;
    }
    let mut settings = toml::Table::new();
    settings.insert("port".to_owned(), toml::Value::Integer(i64::from(port)));
    settings.insert(
        "zeroconf_name".to_owned(),
        toml::Value::String(network.zeroconf_name.clone()),
    );
    Some(block("mdns", "mdns", settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_block_carries_port_and_name() {
        let net = NetworkConfig::default();
        let cfg = mdns_config(&net, &[], 6601).unwrap();
        assert_eq!(cfg.integration_type, "mdns");
        assert_eq!(cfg.setting_str("port").as_deref(), Some("6601"));
        assert_eq!(cfg.setting_str("zeroconf_name").as_deref(), Some("rmpd@%h"));
    }

    #[test]
    fn mdns_respects_disable_and_explicit_block() {
        let mut net = NetworkConfig::default();
        let explicit = [block("m", "MDNS", toml::Table::new())];
        assert!(mdns_config(&net, &explicit, 6600).is_none());
        net.zeroconf_enabled = false;
        assert!(mdns_config(&net, &[], 6600).is_none());
    }

    #[test]
    fn mpris_follows_media_controls() {
        let mut net = NetworkConfig::default();
        net.media_controls = false;
        assert!(synthesize_builtin(&net, &[]).is_empty());
        net.media_controls = true;
        let out = synthesize_builtin(&net, &[]);
        assert_eq!(out.len(), usize::from(cfg!(not(target_os = "macos"))));
        let explicit = [block("x", "mpris", toml::Table::new())];
        assert!(synthesize_builtin(&net, &explicit).is_empty());
    }
}
