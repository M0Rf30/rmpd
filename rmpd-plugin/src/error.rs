// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error type shared by cross-cutting plugin factories and runtimes.

use std::fmt;

/// Error returned by plugin factories and plugin run loops.
///
/// Messages MUST NOT contain credentials or other secrets.
#[derive(Debug)]
pub enum PluginError {
    /// Missing or invalid configuration (bad settings, unknown plugin type).
    Config(String),
    /// The plugin is known but not usable in this build/environment.
    Unavailable(String),
    /// A runtime failure inside the plugin (I/O, network, ...).
    Runtime(String),
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Config(m) => write!(f, "plugin config error: {m}"),
            PluginError::Unavailable(m) => write!(f, "plugin unavailable: {m}"),
            PluginError::Runtime(m) => write!(f, "plugin error: {m}"),
        }
    }
}

impl std::error::Error for PluginError {}
