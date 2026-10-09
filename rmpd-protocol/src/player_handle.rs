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
use rmpd_core::state::PlayerState;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{PlayerHandle, PlayerSnapshot};
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
}
