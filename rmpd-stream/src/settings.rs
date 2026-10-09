// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process-wide `[stream]` settings.
//!
//! The decoder opens inputs from arbitrary threads through a plain
//! `open(path)` call, so the daemon installs the configuration once at startup
//! with [`configure`] and every subsequent connection reads the current value.

use std::sync::{Arc, LazyLock};

use parking_lot::RwLock;
use rmpd_core::config::StreamConfig;

use crate::glob::glob_match;

static SETTINGS: LazyLock<RwLock<Arc<StreamConfig>>> =
    LazyLock::new(|| RwLock::new(Arc::new(StreamConfig::default())));

/// Install the `[stream]` configuration used by all later connections.
pub fn configure(config: &StreamConfig) {
    *SETTINGS.write() = Arc::new(config.clone());
}

/// Snapshot of the active configuration.
#[must_use]
pub(crate) fn current() -> Arc<StreamConfig> {
    Arc::clone(&SETTINGS.read())
}

/// Whether `url` matches any `metadata_blacklist` glob (such streams ignore
/// their in-band ICY `StreamTitle`).
#[must_use]
pub fn is_metadata_blacklisted(patterns: &[String], url: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_blacklist_matches_nothing() {
        assert!(!is_metadata_blacklisted(&[], "http://radio.example/stream"));
    }

    #[test]
    fn blacklist_matches_any_pattern() {
        let p = pats(&["*://ads.example.com/*", "http://radio.example/noisy*"]);
        assert!(is_metadata_blacklisted(&p, "https://ads.example.com/live"));
        assert!(is_metadata_blacklisted(
            &p,
            "http://radio.example/noisy.mp3"
        ));
        assert!(!is_metadata_blacklisted(&p, "http://radio.example/clean"));
    }

    #[test]
    fn default_settings_are_documented_values() {
        let cfg = current();
        assert_eq!(cfg.timeout_ms, 5000);
    }
}
