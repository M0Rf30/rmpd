#![allow(clippy::cargo_common_metadata)]

//! # rmpd plugin SPI (service-provider interface)
//!
//! rmpd follows MPD's *compile-time* plugin model: each category (output,
//! decoder, filter, encoder, mixer, …) is a Rust **trait** plus a `const`
//! name→factory registry, selected at runtime by name and gated by Cargo
//! features. There is intentionally **no** dynamic `.so` loading — Rust has no
//! stable ABI, and MPD itself links all plugins at build time.
//!
//! Concrete plugin traits and registries currently live next to their
//! subsystems (e.g. the audio-output registry in `rmpd-player`). This crate is
//! the future home for the cross-cutting SPI definitions; see
//! `docs/PLUGIN_ARCHITECTURE.md`.
pub mod source;
pub use source::{MusicSource, SourceEntry, SourceError, SourceResult};
