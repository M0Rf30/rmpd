// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration SPI: long-running background plugins (scrobblers, notifiers,
//! remote-control bridges, ...) that observe the player's event stream and
//! may drive playback through a [`PlayerHandle`].
//!
//! Concrete integrations live in the `rmpd-integrations` crate, which owns the
//! compile-time `INTEGRATION_PLUGINS` registry.

use crate::error::PluginError;
use async_trait::async_trait;
use rmpd_core::config::IntegrationConfig;
use rmpd_core::event::Event;
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

/// Queue playback options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerOptions {
    pub repeat: bool,
    pub random: bool,
    /// Single mode is on (including one-shot).
    pub single: bool,
}

// ─── PlayerHandle ────────────────────────────────────────────────────────────

/// Point-in-time view of the player.
#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    pub state: PlayerState,
    pub elapsed: Option<Duration>,
    pub duration: Option<Duration>,
    /// 0..=100.
    pub volume: u8,
    /// Currently loaded queue song, if any.
    pub song: Option<Arc<Song>>,
}

/// Narrow control/inspection surface handed to integrations. Implemented by
/// `rmpd-protocol` over the live server state.
#[async_trait]
pub trait PlayerHandle: Send + Sync {
    /// Snapshot of state, position, volume and current song.
    async fn status(&self) -> PlayerSnapshot;
    async fn play(&self) -> Result<(), PluginError>;
    async fn pause(&self) -> Result<(), PluginError>;
    /// Pause when playing, resume when paused, start when stopped.
    async fn toggle(&self) -> Result<(), PluginError>;
    async fn next(&self) -> Result<(), PluginError>;
    async fn previous(&self) -> Result<(), PluginError>;
    async fn stop(&self) -> Result<(), PluginError>;
    /// `volume` is clamped to 0..=100.
    async fn set_volume(&self, volume: u8) -> Result<(), PluginError>;
    /// Seek within the current song.
    async fn seek(&self, position: Duration) -> Result<(), PluginError>;

    // ── Additive extensions (default impls keep existing handles compiling) ──

    /// Queue id of the current song, if any.
    async fn current_song_id(&self) -> Option<u32> {
        None
    }
    /// Number of songs in the queue.
    async fn queue_len(&self) -> usize {
        0
    }
    /// Repeat / random / single flags.
    async fn options(&self) -> PlayerOptions {
        PlayerOptions::default()
    }
    async fn set_repeat(&self, _on: bool) -> Result<(), PluginError> {
        Err(PluginError::Unavailable("set_repeat".to_owned()))
    }
    async fn set_random(&self, _on: bool) -> Result<(), PluginError> {
        Err(PluginError::Unavailable("set_random".to_owned()))
    }
    /// Enable/disable single mode (stop or repeat-one after the current song).
    async fn set_single(&self, _on: bool) -> Result<(), PluginError> {
        Err(PluginError::Unavailable("set_single".to_owned()))
    }
    /// Seek by `delta_secs` (may be negative) relative to the current position.
    async fn seek_relative(&self, _delta_secs: f64) -> Result<(), PluginError> {
        Err(PluginError::Unavailable("seek_relative".to_owned()))
    }
    /// Live playback position (not older than the last periodic event).
    async fn position(&self) -> Option<Duration> {
        self.status().await.elapsed
    }
    /// Configured music directory, used to build `file://` URIs.
    fn music_dir(&self) -> Option<String> {
        None
    }
    /// Ask the daemon to shut down.
    fn request_shutdown(&self) {}
}

// ─── Shutdown signalling ─────────────────────────────────────────────────────

/// Sending half of a shutdown signal.
#[derive(Debug)]
pub struct ShutdownTrigger(watch::Sender<bool>);

/// Receiving half: cheap to clone, observed by integrations.
#[derive(Debug, Clone)]
pub struct ShutdownSignal(watch::Receiver<bool>);

/// Create a linked trigger/signal pair.
#[must_use]
pub fn shutdown_channel() -> (ShutdownTrigger, ShutdownSignal) {
    let (tx, rx) = watch::channel(false);
    (ShutdownTrigger(tx), ShutdownSignal(rx))
}

impl ShutdownTrigger {
    /// Request shutdown. Idempotent.
    pub fn trigger(&self) {
        self.0.send_replace(true);
    }
}

impl ShutdownSignal {
    /// True once shutdown was requested (or the trigger was dropped).
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        *self.0.borrow() || self.0.has_changed().is_err()
    }

    /// Resolve when shutdown is requested or the trigger is dropped.
    pub async fn cancelled(&mut self) {
        loop {
            if *self.0.borrow_and_update() {
                return;
            }
            if self.0.changed().await.is_err() {
                return;
            }
        }
    }
}

// ─── Integration ─────────────────────────────────────────────────────────────

/// Everything an integration needs at run time.
pub struct IntegrationContext {
    /// Subscription to the global event bus (taken before the integration
    /// starts, so no events are missed).
    pub events: broadcast::Receiver<Event>,
    /// Player inspection/control handle.
    pub player: Arc<dyn PlayerHandle>,
    /// Directory the integration may persist state in (already created).
    pub state_dir: PathBuf,
    /// Fires when the daemon is shutting down; `run` should return promptly.
    pub shutdown: ShutdownSignal,
}

/// A background integration. Constructed synchronously by a factory (no I/O),
/// then driven by `run` on its own Tokio task until shutdown.
#[async_trait]
pub trait Integration: Send {
    /// Instance name (`[[integration]] name =`), used in logs.
    fn name(&self) -> &str;

    /// Main loop. MUST return when `ctx.shutdown` fires. An `Err` is logged;
    /// it never takes the daemon down.
    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError>;
}

/// Sync, no-I/O factory: build an integration from its `[[integration]]`
/// block or fail with [`PluginError::Config`].
pub type IntegrationFactory = fn(&IntegrationConfig) -> Result<Box<dyn Integration>, PluginError>;

/// One registry entry for an integration type.
#[derive(Clone, Copy)]
pub struct IntegrationPlugin {
    /// Matches `[[integration]] type =` (lowercase).
    pub name: &'static str,
    /// Setting keys the plugin accepts; others produce a warning.
    pub settings: &'static [&'static str],
    pub factory: IntegrationFactory,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_signal_fires() {
        let (trigger, mut signal) = shutdown_channel();
        assert!(!signal.is_shutdown());
        trigger.trigger();
        signal.cancelled().await;
        assert!(signal.is_shutdown());
    }

    #[tokio::test]
    async fn dropped_trigger_cancels() {
        let (trigger, mut signal) = shutdown_channel();
        drop(trigger);
        signal.cancelled().await;
    }
}
