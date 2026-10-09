// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::cargo_common_metadata)]

pub mod commands;
pub mod connection;
pub mod discovery;
pub(crate) mod helpers;
// Every platform with a session bus gets the MPRIS interface; macOS has none and
// uses the native Now Playing integration instead.
#[cfg(not(target_os = "macos"))]
pub mod mpris;

#[cfg(target_os = "macos")]
pub mod media_controls_macos;
pub mod now_playing_art;
pub mod parser;
pub mod player_handle;
pub mod queue_playback;
pub mod response;
pub mod server;
pub mod state;
pub mod statefile;

pub use connection::ConnectionState;
pub use player_handle::ServerPlayerHandle;
pub use queue_playback::QueuePlaybackManager;
pub use server::MpdServer;
pub use state::AppState;
pub use statefile::StateFile;
