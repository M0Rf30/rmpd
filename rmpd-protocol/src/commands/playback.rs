//! Playback control command handlers

use tracing::{debug, error, info};

use crate::response::ResponseBuilder;
use crate::state::AppState;

/// Resolve relative path to absolute path using music_directory
/// If path is already absolute, return as-is
fn resolve_path(rel_path: &str, music_dir: Option<&str>) -> String {
    // If path is already absolute, return as-is
    if rel_path.starts_with('/') {
        return rel_path.to_string();
    }

    // Otherwise, prepend music_directory
    if let Some(music_dir) = music_dir {
        let music_dir = music_dir.trim_end_matches('/');
        format!("{}/{}", music_dir, rel_path)
    } else {
        // Fallback: return as-is if no music_dir
        rel_path.to_string()
    }
}

pub async fn handle_play_command(state: &AppState, position: Option<u32>) -> String {
    let queue = state.queue.read().await;

    // Get song to play and track the actual position
    let (song, actual_position) = if let Some(pos) = position {
        // Play specific position
        if let Some(item) = queue.get(pos) {
            (item.song.clone(), Some((pos, item.id)))
        } else {
            return ResponseBuilder::error(50, 0, "play", "No such song");
        }
    } else {
        // Resume or play first song
        let current_song = state.engine.read().await.get_current_song().await;
        if let Some(song) = current_song {
            // Resuming - keep existing position if set
            let pos = state.status.read().await.current_song;
            (song, pos.map(|p| (p.position, p.id)))
        } else if let Some(item) = queue.get(0) {
            // Play first song
            (item.song.clone(), Some((0, item.id)))
        } else {
            return ResponseBuilder::error(50, 0, "play", "No songs in queue");
        }
    };

    drop(queue);

    // Resolve relative path to absolute for playback
    let mut playback_song = song.clone();
    let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
    playback_song.path = absolute_path.into();

    // Start playback with resolved path
    match state.engine.write().await.play(playback_song).await {
        Ok(_) => {
            // Update status immediately (event will also update but that's idempotent)
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Play;
            status.elapsed = Some(std::time::Duration::ZERO);
            status.duration = song.duration;
            status.bitrate = song.bitrate;

            // Set audio format if available
            if let (Some(sr), Some(ch), Some(bps)) =
                (song.sample_rate, song.channels, song.bits_per_sample)
            {
                status.audio_format = Some(rmpd_core::song::AudioFormat {
                    sample_rate: sr,
                    channels: ch,
                    bits_per_sample: bps,
                });
            }

            if let Some((pos, id)) = actual_position {
                status.current_song = Some(rmpd_core::state::QueuePosition { position: pos, id });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }
            }
            drop(status);

            // Emit events to notify idle clients
            debug!("Emitting PlayerStateChanged(Play) and SongChanged events");
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(
                    rmpd_core::state::PlayerState::Play,
                ));
            state
                .event_bus
                .emit(rmpd_core::event::Event::SongChanged(Some(song)));

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "play", &format!("Playback error: {}", e)),
    }
}

pub async fn handle_pause_command(state: &AppState, pause_state: Option<bool>) -> String {
    info!("Pause command received: pause_state={:?}", pause_state);

    // Get current state lock-free using atomic (no engine lock needed!)
    let current_state_u8 = state.atomic_state.load(std::sync::atomic::Ordering::SeqCst);
    let current_state = match current_state_u8 {
        0 => rmpd_core::state::PlayerState::Stop,
        1 => rmpd_core::state::PlayerState::Play,
        2 => rmpd_core::state::PlayerState::Pause,
        _ => rmpd_core::state::PlayerState::Stop,
    };

    info!("Current state (atomic, no locks): {:?}", current_state);

    let should_pause =
        pause_state.unwrap_or_else(|| current_state == rmpd_core::state::PlayerState::Play);
    let is_currently_paused = current_state == rmpd_core::state::PlayerState::Pause;

    // If already in desired state, do nothing
    if should_pause == is_currently_paused {
        info!("Already in desired state, returning OK");
        return ResponseBuilder::new().ok();
    }

    info!("Acquiring engine write lock...");
    // Set pause state
    let result = if pause_state.is_some() {
        // Explicit pause state given - use set_pause
        state.engine.write().await.set_pause(should_pause).await
    } else {
        // No explicit state - toggle
        state.engine.write().await.pause().await
    };

    match result {
        Ok(_) => {
            info!("Engine pause completed, updating status...");
            // Read the actual state from atomic (engine might not have changed it)
            let actual_state_u8 = state.atomic_state.load(std::sync::atomic::Ordering::SeqCst);
            let actual_state = match actual_state_u8 {
                0 => rmpd_core::state::PlayerState::Stop,
                1 => rmpd_core::state::PlayerState::Play,
                2 => rmpd_core::state::PlayerState::Pause,
                _ => rmpd_core::state::PlayerState::Stop,
            };

            // Update status to match actual atomic state
            let mut status = state.status.write().await;
            status.state = actual_state;
            drop(status);

            // Emit event to notify idle clients
            debug!("Emitting PlayerStateChanged({:?}) event", actual_state);
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(actual_state));

            info!(
                "Pause completed successfully, state is now: {:?}",
                actual_state
            );
            ResponseBuilder::new().ok()
        }
        Err(e) => {
            error!("Pause failed: {}", e);
            ResponseBuilder::error(50, 0, "pause", &format!("Pause error: {}", e))
        }
    }
}

pub async fn handle_stop_command(state: &AppState) -> String {
    info!("Stop command received");
    info!("Acquiring engine write lock for stop...");
    match state.engine.write().await.stop().await {
        Ok(_) => {
            // Update status after engine stops
            let mut status = state.status.write().await;
            status.state = rmpd_core::state::PlayerState::Stop;
            status.current_song = None;
            status.next_song = None;
            drop(status);

            // Emit event to notify idle clients
            debug!("Emitting PlayerStateChanged(Stop) event");
            state
                .event_bus
                .emit(rmpd_core::event::Event::PlayerStateChanged(
                    rmpd_core::state::PlayerState::Stop,
                ));

            ResponseBuilder::new().ok()
        }
        Err(e) => ResponseBuilder::error(50, 0, "stop", &format!("Stop error: {}", e)),
    }
}

pub async fn handle_next_command(state: &AppState) -> String {
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    let next_pos = if let Some(current) = status.current_song {
        current.position + 1
    } else {
        0
    };

    if let Some(item) = queue.get(next_pos) {
        let song = item.song.clone();
        let item_id = item.id;
        drop(queue);
        drop(status);

        // Resolve relative path to absolute for playback
        let mut playback_song = song.clone();
        let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
        playback_song.path = absolute_path.into();

        match state.engine.write().await.play(playback_song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: next_pos,
                    id: item_id,
                });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(next_pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: next_pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }

                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "next", &format!("Playback error: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "next", "No next song")
    }
}

pub async fn handle_previous_command(state: &AppState) -> String {
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    let prev_pos = if let Some(current) = status.current_song {
        if current.position > 0 {
            current.position - 1
        } else {
            return ResponseBuilder::error(50, 0, "previous", "Already at first song");
        }
    } else {
        0
    };

    if let Some(item) = queue.get(prev_pos) {
        let song = item.song.clone();
        let item_id = item.id;
        drop(queue);
        drop(status);

        // Resolve relative path to absolute for playback
        let mut playback_song = song.clone();
        let absolute_path = resolve_path(song.path.as_str(), state.music_dir.as_deref());
        playback_song.path = absolute_path.into();

        match state.engine.write().await.play(playback_song).await {
            Ok(_) => {
                let mut status = state.status.write().await;
                status.current_song = Some(rmpd_core::state::QueuePosition {
                    position: prev_pos,
                    id: item_id,
                });

                // Set next_song for UI (e.g., Cantata's next button)
                let queue = state.queue.read().await;
                if let Some(next_item) = queue.get(prev_pos + 1) {
                    status.next_song = Some(rmpd_core::state::QueuePosition {
                        position: prev_pos + 1,
                        id: next_item.id,
                    });
                } else {
                    status.next_song = None;
                }

                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "previous", &format!("Playback error: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "previous", "No previous song")
    }
}

pub async fn handle_seek_command(state: &AppState, position: u32, time: f64) -> String {
    // Get song at position
    let queue = state.queue.read().await;
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.position == position {
            drop(queue);
            drop(status);
            // Seek in current song
            match state.engine.read().await.seek(time).await {
                Ok(_) => {
                    // Update status elapsed time
                    state.status.write().await.elapsed =
                        Some(std::time::Duration::from_secs_f64(time));
                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "seek", &format!("Seek failed: {}", e)),
            }
        } else {
            ResponseBuilder::error(50, 0, "seek", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seek", "Not playing")
    }
}

pub async fn handle_seekid_command(state: &AppState, id: u32, time: f64) -> String {
    let status = state.status.read().await;

    // Check if this is the current song
    if let Some(current) = status.current_song {
        if current.id == id {
            drop(status);
            // Seek in current song
            match state.engine.read().await.seek(time).await {
                Ok(_) => {
                    // Update status elapsed time
                    state.status.write().await.elapsed =
                        Some(std::time::Duration::from_secs_f64(time));
                    ResponseBuilder::new().ok()
                }
                Err(e) => ResponseBuilder::error(50, 0, "seekid", &format!("Seek failed: {}", e)),
            }
        } else {
            ResponseBuilder::error(50, 0, "seekid", "Can only seek in current song")
        }
    } else {
        ResponseBuilder::error(50, 0, "seekid", "Not playing")
    }
}

pub async fn handle_seekcur_command(state: &AppState, time: f64, relative: bool) -> String {
    let status = state.status.read().await;

    if status.current_song.is_some() {
        let current_elapsed = status
            .elapsed
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs_f64();
        drop(status);

        // Calculate actual seek position
        let seek_position = if relative {
            // Relative seek: add to current position
            (current_elapsed + time).max(0.0)
        } else {
            // Absolute seek
            time.max(0.0)
        };

        // Seek in current song
        match state.engine.read().await.seek(seek_position).await {
            Ok(_) => {
                // Update status elapsed time
                state.status.write().await.elapsed =
                    Some(std::time::Duration::from_secs_f64(seek_position));
                ResponseBuilder::new().ok()
            }
            Err(e) => ResponseBuilder::error(50, 0, "seekcur", &format!("Seek failed: {}", e)),
        }
    } else {
        ResponseBuilder::error(50, 0, "seekcur", "Not playing")
    }
}
