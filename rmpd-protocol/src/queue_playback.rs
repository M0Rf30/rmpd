use crate::commands::utils::prepare_song_for_playback;
use crate::helpers;
use crate::state::AppState;
use rmpd_core::event::Event;
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
                    Ok(Event::StreamTitleChanged(title)) => {
                        debug!("stream title changed to: {:?}", title);
                        *state.stream_title.write().await = title;
                    }
                    Ok(Event::AdvancedToNext) => {
                        info!("engine advanced to next song in-thread (gapless/crossfade)");
                        if let Err(e) = Self::handle_advanced(&state).await {
                            error!("error handling in-thread advance: {}", e);
                        }
                        Self::feed_next_song(&state).await;
                    }
                    Ok(Event::SongChanged(_)) => {
                        // A new song invalidates any prior stream title.
                        *state.stream_title.write().await = None;
                        // (Re)feed look-ahead whenever the current song changes — covers
                        // manual play/playid, resume, and the SongFinished fallback.
                        Self::feed_next_song(&state).await;
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
    async fn handle_song_finished(state: &AppState) -> rmpd_core::error::Result<()> {
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
                use rand::RngExt;
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
                    helpers::update_player_state(state, PlayerState::Stop).await;
                    state.status.write().await.current_song = None;
                    return Ok(());
                }
            } else {
                next
            }
        };

        // Get the next song
        if let Some(item) = queue.get(next_pos) {
            let song = (*item.song).clone();
            let item_id = item.id;
            drop(queue);

            // Handle consume mode (remove current song after playing)
            if consume.is_on() {
                state.queue.write().await.delete(current_pos);
                // Notify the `playlist` idle subsystem that the consumed song was
                // removed from the queue.
                helpers::update_playlist_version(state).await;
            }

            // Play the next song
            let playback_song = prepare_song_for_playback(&song, state.music_dir.as_deref());
            match state.engine.write().await.play(playback_song).await {
                Ok(_) => {
                    let mut status = state.status.write().await;
                    status.state = PlayerState::Play;
                    status.elapsed = Some(Duration::ZERO);
                    status.duration = song.duration;
                    status.bitrate = song.bitrate;
                    status.audio_format = helpers::extract_audio_format(&song);

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

                    state
                        .event_bus
                        .emit(Event::PlayerStateChanged(PlayerState::Play));
                    state.event_bus.emit(Event::SongChanged(Some(song)));

                    if should_stop_after {
                        state.engine.write().await.stop().await?;
                        helpers::update_player_state(state, PlayerState::Stop).await;
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

    /// Returns the next position to look ahead to, or None when look-ahead must be
    /// disabled (random, single engaged, or end-of-queue without repeat).
    fn lookahead_next_pos(
        current_pos: u32,
        queue_len: u32,
        repeat: bool,
        random: bool,
        single: rmpd_core::state::SingleMode,
    ) -> Option<u32> {
        if random || single.is_on() || single.is_oneshot() || queue_len == 0 {
            return None;
        }
        let next = current_pos + 1;
        if next >= queue_len {
            if repeat { Some(0) } else { None }
        } else {
            Some(next)
        }
    }

    /// Feed the engine the upcoming song for gapless/crossfade look-ahead.
    pub async fn feed_next_song(state: &AppState) {
        let (current_pos, repeat, random, single) = {
            let status = state.status.read().await;
            match status.current_song {
                Some(ref p) => (p.position, status.repeat, status.random, status.single),
                None => {
                    drop(status);
                    state.engine.read().await.set_next_song(None);
                    return;
                }
            }
        };
        let next_ps = {
            let queue = state.queue.read().await;
            match Self::lookahead_next_pos(current_pos, queue.len() as u32, repeat, random, single)
            {
                Some(np) => queue.get(np).map(|item| {
                    prepare_song_for_playback(&(*item.song).clone(), state.music_dir.as_deref())
                }),
                None => None,
            }
        };
        state.engine.read().await.set_next_song(next_ps);
    }

    /// Handle in-thread advance event — the engine already started the next song
    /// gaplessly/via crossfade; we only update bookkeeping (no engine.play call).
    async fn handle_advanced(state: &AppState) -> rmpd_core::error::Result<()> {
        let (current_pos, repeat, random, single, consume) = {
            let s = state.status.read().await;
            match s.current_song {
                Some(ref p) => (p.position, s.repeat, s.random, s.single, s.consume),
                None => return Ok(()),
            }
        };
        let next_pos = match Self::lookahead_next_pos(
            current_pos,
            state.queue.read().await.len() as u32,
            repeat,
            random,
            single,
        ) {
            Some(np) => np,
            None => return Ok(()), // shouldn't happen — engine only advances when we fed
        };
        let (song, item_id) = {
            let q = state.queue.read().await;
            match q.get(next_pos) {
                Some(i) => ((*i.song).clone(), i.id),
                None => return Ok(()),
            }
        };
        if consume.is_on() {
            state.queue.write().await.delete(current_pos);
            helpers::update_playlist_version(state).await;
        }
        {
            let mut status = state.status.write().await;
            status.state = PlayerState::Play;
            status.elapsed = Some(Duration::ZERO);
            status.duration = song.duration;
            status.bitrate = song.bitrate;
            status.audio_format = helpers::extract_audio_format(&song);
            status.current_song = Some(QueuePosition {
                position: if consume.is_on() && next_pos > current_pos {
                    next_pos - 1
                } else {
                    next_pos
                },
                id: item_id,
            });
            if single.is_oneshot() {
                status.single = rmpd_core::state::SingleMode::Off;
            }
            if consume.is_oneshot() {
                status.consume = rmpd_core::state::ConsumeMode::Off;
            }
        }
        state.event_bus.emit(Event::SongChanged(Some(song)));
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
