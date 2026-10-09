// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`PlayerHandle`] implementation over the live server [`AppState`], handed
//! to integrations (see `rmpd_plugin::integration`).
//!
//! Controls go through the same command handlers MPD clients hit, so
//! idle events, queue semantics and error handling are identical.

use crate::commands::{options, playback};
use crate::state::AppState;
use async_trait::async_trait;
use rmpd_core::state::{PlayerState, SingleMode};
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{PlayerHandle, PlayerOptions, PlayerSnapshot};
use std::time::Duration;

/// Server-backed [`PlayerHandle`].
#[derive(Clone)]
pub struct ServerPlayerHandle {
    state: AppState,
}

impl ServerPlayerHandle {
    #[must_use]
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Map a protocol response string (`OK\n` / `ACK ...`) to a `Result`.
fn check(response: &str) -> Result<(), PluginError> {
    if response.starts_with("ACK") {
        Err(PluginError::Runtime(response.trim().to_owned()))
    } else {
        Ok(())
    }
}

#[async_trait]
impl PlayerHandle for ServerPlayerHandle {
    async fn status(&self) -> PlayerSnapshot {
        let status = self.state.status.read().await.clone();
        let song = match status.current_song {
            Some(pos) => self
                .state
                .queue
                .read()
                .await
                .get_by_id(pos.id)
                .map(|item| item.song.clone()),
            None => None,
        };
        PlayerSnapshot {
            state: status.state,
            elapsed: status.elapsed,
            duration: status.duration,
            volume: status.volume,
            song,
        }
    }

    async fn play(&self) -> Result<(), PluginError> {
        check(&playback::handle_play_command(&self.state, None).await)
    }

    async fn pause(&self) -> Result<(), PluginError> {
        check(&playback::handle_pause_command(&self.state, Some(true)).await)
    }

    async fn toggle(&self) -> Result<(), PluginError> {
        if self.status().await.state == PlayerState::Stop {
            self.play().await
        } else {
            check(&playback::handle_pause_command(&self.state, None).await)
        }
    }

    async fn next(&self) -> Result<(), PluginError> {
        check(&playback::handle_next_command(&self.state).await)
    }

    async fn previous(&self) -> Result<(), PluginError> {
        check(&playback::handle_previous_command(&self.state).await)
    }

    async fn stop(&self) -> Result<(), PluginError> {
        check(&playback::handle_stop_command(&self.state).await)
    }

    async fn set_volume(&self, volume: u8) -> Result<(), PluginError> {
        check(&options::handle_setvol_command(&self.state, volume.min(100)).await)
    }

    async fn seek(&self, position: Duration) -> Result<(), PluginError> {
        check(&playback::handle_seekcur_command(&self.state, position.as_secs_f64(), false).await)
    }

    async fn current_song_id(&self) -> Option<u32> {
        self.state.status.read().await.current_song.map(|p| p.id)
    }

    async fn queue_len(&self) -> usize {
        self.state.queue.read().await.len()
    }

    async fn options(&self) -> PlayerOptions {
        let status = self.state.status.read().await;
        PlayerOptions {
            repeat: status.repeat,
            random: status.random,
            single: status.single != SingleMode::Off,
        }
    }

    async fn set_repeat(&self, on: bool) -> Result<(), PluginError> {
        check(&options::handle_repeat_command(&self.state, on).await)
    }

    async fn set_random(&self, on: bool) -> Result<(), PluginError> {
        check(&options::handle_random_command(&self.state, on).await)
    }

    async fn set_single(&self, on: bool) -> Result<(), PluginError> {
        check(&options::handle_single_command(&self.state, if on { "1" } else { "0" }).await)
    }

    async fn seek_relative(&self, delta_secs: f64) -> Result<(), PluginError> {
        check(&playback::handle_seekcur_command(&self.state, delta_secs, true).await)
    }

    async fn position(&self) -> Option<Duration> {
        // Live at query time so clients don't read a position up to ~1s stale.
        let live = self.state.engine.read().await.get_elapsed_live();
        match live {
            Some(d) => Some(d),
            None => self.state.status.read().await.elapsed,
        }
    }

    fn music_dir(&self) -> Option<String> {
        self.state.music_dir.clone()
    }

    fn request_shutdown(&self) {
        if let Some(tx) = &self.state.shutdown_tx {
            let _ = tx.send(());
        }
    }
}
