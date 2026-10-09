// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! mDNS/Zeroconf advertisement integration.
//!
//! Registers a `_mpd._tcp.local.` service so MPD clients on the LAN can
//! auto-discover rmpd. The service lives as long as the integration runs and
//! is unregistered on shutdown.
//!
//! Enabled implicitly by `network.zeroconf_enabled` (the daemon synthesizes
//! the block once the TCP listener is bound, because the advertised port is
//! only known then), or explicitly with `[[integration]] type = "mdns"`.
//!
//! Settings: `port` (required, 1-65535) and `zeroconf_name` (instance-name
//! template, default `rmpd@%h`, where `%h` expands to the hostname).

use async_trait::async_trait;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rmpd_core::config::IntegrationConfig;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{Integration, IntegrationContext, IntegrationPlugin};
use tracing::{info, warn};

/// Settings accepted by the mDNS integration.
pub const SETTINGS: &[&str] = &["port", "zeroconf_name"];

/// Registry entry.
pub const PLUGIN: IntegrationPlugin = IntegrationPlugin {
    name: "mdns",
    settings: SETTINGS,
    factory,
};

/// Default instance-name template (matches `network.zeroconf_name`).
pub const DEFAULT_NAME: &str = "rmpd@%h";

const SERVICE_TYPE: &str = "_mpd._tcp.local.";

fn factory(cfg: &IntegrationConfig) -> Result<Box<dyn Integration>, PluginError> {
    let port = cfg
        .setting_str("port")
        .ok_or_else(|| PluginError::Config("mdns integration requires `port`".to_owned()))?;
    let port: u16 = port
        .parse()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| PluginError::Config("mdns `port` must be 1-65535".to_owned()))?;
    let template = cfg
        .setting_str("zeroconf_name")
        .unwrap_or_else(|| DEFAULT_NAME.to_owned());
    Ok(Box::new(MdnsIntegration {
        name: cfg.name.clone(),
        port,
        template,
    }))
}

/// The mDNS advertiser.
pub struct MdnsIntegration {
    name: String,
    port: u16,
    template: String,
}

/// Expand `%h` in the instance-name template.
fn instance_name(template: &str, hostname: &str) -> String {
    template.replace("%h", hostname)
}

/// Normalise the raw hostname file contents (fall back to `rmpd`).
fn clean_hostname(raw: &str) -> String {
    let h = raw.trim();
    if h.is_empty() {
        "rmpd".to_owned()
    } else {
        h.to_owned()
    }
}

#[async_trait]
impl Integration for MdnsIntegration {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError> {
        let mut shutdown = ctx.shutdown;

        let hostname =
            clean_hostname(&std::fs::read_to_string("/etc/hostname").unwrap_or_default());
        let instance = instance_name(&self.template, &hostname);
        let host_name = format!("{hostname}.local.");

        let daemon = ServiceDaemon::new()
            .map_err(|e| PluginError::Unavailable(format!("mDNS daemon: {e}")))?;
        let info = ServiceInfo::new(SERVICE_TYPE, &instance, &host_name, (), self.port, None)
            .map_err(|e| PluginError::Runtime(format!("mDNS service info: {e}")))?;
        let fullname = info.get_fullname().to_owned();
        daemon
            .register(info)
            .map_err(|e| PluginError::Runtime(format!("mDNS advertisement failed: {e}")))?;
        info!("advertising rmpd as '{}' on port {}", instance, self.port);

        shutdown.cancelled().await;

        if let Err(e) = daemon.unregister(&fullname) {
            warn!("mDNS unregister failed: {e}");
        }
        if let Err(e) = daemon.shutdown() {
            warn!("mDNS shutdown failed: {e}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(settings: &str) -> IntegrationConfig {
        let mut table: toml::Table = toml::from_str(settings).unwrap();
        table.insert("name".into(), "mdns".into());
        table.insert("type".into(), "mdns".into());
        table.try_into().unwrap()
    }

    #[test]
    fn expands_hostname() {
        assert_eq!(instance_name("rmpd@%h", "box"), "rmpd@box");
        assert_eq!(instance_name("plain", "box"), "plain");
    }

    #[test]
    fn hostname_fallback() {
        assert_eq!(clean_hostname("  \n"), "rmpd");
        assert_eq!(clean_hostname("host\n"), "host");
    }

    #[test]
    fn factory_requires_valid_port() {
        assert!(factory(&cfg("")).is_err());
        assert!(factory(&cfg("port = 0")).is_err());
        assert!(factory(&cfg("port = 99999")).is_err());
        assert!(factory(&cfg("port = 6600")).is_ok());
    }
}
