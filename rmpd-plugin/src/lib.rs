// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::cargo_common_metadata)]

//! # rmpd plugin SPI (service-provider interface)
//!
//! rmpd follows MPD's *compile-time* plugin model: each category (output,
//! decoder, filter, encoder, mixer, …) is a Rust **trait** plus a `const`
//! name→factory registry, selected at runtime by name and gated by Cargo
//! features. There is intentionally **no** dynamic `.so` loading — Rust has no
//! stable ABI, and MPD itself links all plugins at build time.
//!
//! Cross-cutting SPI definitions live here: music sources, playlist parsers,
//! integrations. Subsystem-local SPIs (outputs, mixers, encoders) stay next
//! to their subsystem in `rmpd-player`. See `docs/PLUGIN_ARCHITECTURE.md`.
pub mod error;
pub mod integration;
pub mod playlist;
pub mod source;

pub use error::PluginError;
pub use integration::{
    Integration, IntegrationContext, IntegrationFactory, IntegrationPlugin, PlayerHandle,
    PlayerSnapshot, ShutdownSignal, ShutdownTrigger, shutdown_channel,
};
pub use playlist::{PLAYLIST_PLUGINS, PlaylistEntry, PlaylistParser};
pub use source::{MusicSource, SourceEntry, SourceError, SourceResult, SyncPolicy};
