use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::state::{PlayerState, QueuePosition};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// Queue playback manager that handles automatic song advancement
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
                        info!("Song finished, advancing to next");
                        if let Err(e) = Self::handle_song_finished(&state).await {
                            error!("Error advancing to next song: {}", e);
                        }
                    }
                    Ok(Event::PositionChanged(elapsed)) => {
                        // Update status with current position
                        let mut status = state.status.write().await;
                        status.elapsed = Some(elapsed);
                        // duration is set when play command is issued
                    }
                    Ok(Event::BitrateChanged(bitrate)) => {
                        // Update status with current instantaneous bitrate (VBR support)
                        info!("Bitrate changed to: {:?} kbps", bitrate);
                        let mut status = state.status.write().await;
                        status.bitrate = bitrate;
                    }
                    Ok(_) => {} // Ignore other events
                    Err(e) => {
                        error!("Event receive error: {}", e);
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
                    debug!("End of queue reached, stopping playback");
                    drop(queue);
                    state.engine.write().await.stop().await?;
                    let mut status = state.status.write().await;
                    status.state = PlayerState::Stop;
                    status.current_song = None;
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
            match state.engine.write().await.play(song).await {
                Ok(_) => {
                    let mut status = state.status.write().await;
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

                    // Stop after playing if single mode is on
                    if should_stop_after {
                        state.engine.write().await.stop().await?;
                        let mut status = state.status.write().await;
                        status.state = PlayerState::Stop;
                    }
                }
                Err(e) => {
                    error!("Failed to play next song: {}", e);
                }
            }
        } else {
            debug!("No next song found");
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
