use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::song::AudioFormat;
use rmpd_core::state::{PlayerState, QueuePosition};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// Queue playback manager that handles automatic song advancement
#[derive(Debug)]
pub struct QueuePlaybackManager {
    state: AppState,
    event_task: Option<JoinHandle<()>>,
}

impl QueuePlaybackManager {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            event_task: None,
        }
    }

    /// Start listening for playback events
    pub fn start(&mut self) {
        let state = self.state.clone();
        let mut event_rx = state.event_bus.subscribe();

        let task = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(Event::SongFinished) => {
                        info!("song finished, advancing to next");
                        if let Err(e) = Self::handle_song_finished(&state).await {
                            error!("error advancing to next song: {}", e);
                        }
                    }
                    Ok(Event::PositionChanged(elapsed)) => {
                        // Update status with current position and sync state
                        let mut status = state.status.write().await;
                        status.elapsed = Some(elapsed);

                        // Sync status.state with atomic_state to ensure consistency
                        // Read atomic_state WHILE holding the lock to avoid races
                        let atomic_player_state = rmpd_core::state::PlayerState::from_atomic(
                            state
                                .atomic_state
                                .load(std::sync::atomic::Ordering::Acquire),
                        );

                        let state_changed = status.state != atomic_player_state;
                        if state_changed {
                            debug!(
                                "syncing status.state {:?} -> {:?}",
                                status.state, atomic_player_state
                            );
                            status.state = atomic_player_state;
                        }

                        // Drop lock before emitting event to avoid holding lock during event dispatch
                        drop(status);

                        if state_changed {
                            // Emit PlayerStateChanged event to notify idle clients
                            state
                                .event_bus
                                .emit(Event::PlayerStateChanged(atomic_player_state));
                        }
                    }
                    Ok(Event::BitrateChanged(bitrate)) => {
                        // Update status with current instantaneous bitrate (VBR support)
                        debug!("bitrate changed to: {:?} kbps", bitrate);
                        let mut status = state.status.write().await;
                        status.bitrate = bitrate;
                    }
                    Ok(_) => {} // Ignore other events
                    Err(e) => {
                        error!("event receive error: {}", e);
                        break;
                    }
                }
            }
        });

        self.event_task = Some(task);
    }

    /// Stop the playback manager
    pub fn stop(&mut self) {
        if let Some(task) = self.event_task.take() {
            task.abort();
        }
    }

    /// Handle song finished event - advance to next song
    async fn handle_song_finished(state: &AppState) -> anyhow::Result<()> {
        let status = state.status.read().await;
        let queue = state.queue.read().await;

        // Get current song position
        let current_pos = match status.current_song {
            Some(ref pos) => pos.position,
            None => return Ok(()), // No current song, nothing to do
        };

        // Check playback modes
        let repeat = status.repeat;
        let random = status.random;
        let single = status.single;
        let consume = status.consume;

        drop(status);

        // Determine next position
        let queue_len = queue.len() as u32;
        let next_pos = if random {
            // Random mode: pick a random song
            if queue_len > 0 {
                use rand::Rng;
                let mut rng = rand::rng();
                rng.random_range(0..queue_len)
            } else {
                return Ok(());
            }
        } else {
            // Sequential mode
            let next = current_pos + 1;

            if next >= queue_len {
                // Reached end of queue
                if repeat {
                    // Repeat mode: go back to start
                    0
                } else {
                    // No repeat: stop playback
                    debug!("end of queue reached, stopping playback");
                    drop(queue);
                    state.engine.write().await.stop().await?;
                    // Update status immediately (event will also update but that's idempotent)
                    let mut status = state.status.write().await;
                    status.state = PlayerState::Stop;
                    status.current_song = None;
                    drop(status);
                    // Emit event to notify idle clients
                    state
                        .event_bus
                        .emit(Event::PlayerStateChanged(PlayerState::Stop));
                    return Ok(());
                }
            } else {
                next
            }
        };

        // Get the next song
        if let Some(item) = queue.get(next_pos) {
            let song = item.song.clone();
            let item_id = item.id;
            drop(queue);

            // Handle consume mode (remove current song after playing)
            if consume.is_on() {
                let mut queue = state.queue.write().await;
                queue.delete(current_pos);
                drop(queue);
            }

            // Play the next song
            match state.engine.write().await.play(song.clone()).await {
                Ok(_) => {
                    let mut status = state.status.write().await;

                    // Update playback info immediately (event will also update but that's idempotent)
                    status.state = PlayerState::Play;
                    status.elapsed = Some(Duration::ZERO);
                    status.duration = song.duration;
                    status.bitrate = song.bitrate;

                    // Set audio format if available
                    if let (Some(sr), Some(ch), Some(bps)) =
                        (song.sample_rate, song.channels, song.bits_per_sample)
                    {
                        status.audio_format = Some(AudioFormat {
                            sample_rate: sr,
                            channels: ch,
                            bits_per_sample: bps,
                        });
                    }

                    status.current_song = Some(QueuePosition {
                        position: if consume.is_on() && next_pos > current_pos {
                            // Adjust position if we deleted a song before it
                            next_pos - 1
                        } else {
                            next_pos
                        },
                        id: item_id,
                    });

                    // Handle single mode
                    let should_stop_after = single.is_on();

                    if single.is_oneshot() {
                        // Single oneshot: play one more song then stop
                        status.single = rmpd_core::state::SingleMode::Off;
                    }

                    // Handle consume oneshot
                    if consume.is_oneshot() {
                        status.consume = rmpd_core::state::ConsumeMode::Off;
                    }

                    drop(status);

                    // Emit events to notify idle clients
                    state
                        .event_bus
                        .emit(Event::PlayerStateChanged(PlayerState::Play));
                    state.event_bus.emit(Event::SongChanged(Some(song)));

                    // Stop after playing if single mode is on
                    if should_stop_after {
                        state.engine.write().await.stop().await?;
                        let mut status = state.status.write().await;
                        status.state = PlayerState::Stop;
                        drop(status);
                        // Emit event to notify idle clients
                        state
                            .event_bus
                            .emit(Event::PlayerStateChanged(PlayerState::Stop));
                    }
                }
                Err(e) => {
                    error!("failed to play next song: {}", e);
                }
            }
        } else {
            debug!("no next song found");
        }

        Ok(())
    }
}

impl Drop for QueuePlaybackManager {
    fn drop(&mut self) {
        self.stop();
    }
}

// Helper trait for SingleMode and ConsumeMode
trait ModeExt {
    fn is_on(&self) -> bool;
    fn is_oneshot(&self) -> bool;
}

impl ModeExt for rmpd_core::state::SingleMode {
    fn is_on(&self) -> bool {
        matches!(self, rmpd_core::state::SingleMode::On)
    }

    fn is_oneshot(&self) -> bool {
        matches!(self, rmpd_core::state::SingleMode::Oneshot)
    }
}

impl ModeExt for rmpd_core::state::ConsumeMode {
    fn is_on(&self) -> bool {
        matches!(self, rmpd_core::state::ConsumeMode::On)
    }

    fn is_oneshot(&self) -> bool {
        matches!(self, rmpd_core::state::ConsumeMode::Oneshot)
    }
}
