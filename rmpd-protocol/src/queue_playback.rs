// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::commands::utils::{prepare_song_for_playback, update_next_song};
use crate::helpers;
use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::queue::Queue;
use rmpd_core::state::{ConsumeMode, PlayerState, QueuePosition};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Next queue position to play once the song at `current_pos` is done, or
/// `None` when playback must stop (end of the queue).
///
/// Mirrors MPD's `Queue::GetNextOrder` (`src/queue/Queue.cxx`) — notably its
/// consume rule: with `repeat` **and** `consume`, the queue only wraps to the
/// first song when the finished song is not itself the first one, because
/// consume is about to remove it; wrapping onto a song that is being consumed
/// would replay (and then delete) the very song that just finished.
///
/// In random mode a song is picked (weighted by priority) among all the
/// others; `repeat` allows replaying the only remaining song, again unless
/// `consume` is removing it.
pub(crate) fn next_position(
    queue: &Queue,
    current_pos: u32,
    repeat: bool,
    random: bool,
    consume: bool,
) -> Option<u32> {
    let queue_len = queue.len() as u32;
    if random {
        match queue.weighted_random_pos(Some(current_pos)) {
            Some(pos) => Some(pos),
            None if repeat && !consume && queue_len > 0 => queue.weighted_random_pos(None),
            None => None,
        }
    } else {
        let next = current_pos + 1;
        if next < queue_len {
            Some(next)
        } else if repeat && queue_len > 0 && (current_pos > 0 || !consume) {
            Some(0)
        } else {
            None
        }
    }
}

/// Apply MPD's consume mode to the song that just finished (`playlist::PlayNext`
/// / `QueuedSongStarted`): any mode but "off" removes it from the queue, and
/// "oneshot" then switches consume off again.
///
/// The song is removed by id, so the result does not depend on the position
/// the status last recorded for it.
pub(crate) async fn consume_finished(state: &AppState, finished: QueuePosition) {
    let mode = state.status.read().await.consume;
    if mode == ConsumeMode::Off {
        return;
    }
    let removed = state.queue.write().await.delete_id(finished.id).is_some();
    if removed {
        helpers::update_playlist_version(state).await;
    }
    if mode == ConsumeMode::Oneshot {
        state.status.write().await.consume = ConsumeMode::Off;
        state.event_bus.emit(Event::QueueOptionsChanged);
    }
}

/// Stop the engine and mark the player stopped.
///
/// `clear_current` distinguishes MPD's two ways of getting to "stopped":
/// `playlist::Stop` keeps `current` (the `stop` command, a playback error),
/// whereas running off the end of the queue also resets it (`PlayNext`:
/// `current = -1`).
pub(crate) async fn stop_playback(
    state: &AppState,
    clear_current: bool,
) -> rmpd_core::error::Result<()> {
    stop_playback_guarded(state, clear_current, None)
        .await
        .map(|_| ())
}

/// [`stop_playback`], but only if the engine is still in playback
/// `generation` `expected` (checked under the engine lock, so it cannot race a
/// `play`/`stop` from a client). Returns `false`, having done nothing, when
/// the engine has moved on — i.e. the event being handled is stale.
pub(crate) async fn stop_playback_guarded(
    state: &AppState,
    clear_current: bool,
    expected: Option<u64>,
) -> rmpd_core::error::Result<bool> {
    {
        let mut engine = state.engine.write().await;
        if expected.is_some_and(|g| engine.generation() != g) {
            return Ok(false);
        }
        engine.stop().await?;
    }
    // One critical section: `status` syncs `state` from the engine's (already
    // stopped) atomic under this same lock, so a separate state/current update
    // would let a client see "stop" with the old current song.
    {
        let mut status = state.status.write().await;
        status.state = PlayerState::Stop;
        status.elapsed = None;
        if clear_current {
            status.current_song = None;
            status.next_song = None;
        }
    }
    state
        .event_bus
        .emit(Event::PlayerStateChanged(PlayerState::Stop));
    Ok(true)
}

/// Re-anchor `status.current_song` / `next_song` on the queue after it was
/// edited (insert, delete, move, shuffle, ...), the way MPD keeps
/// `playlist::current` pointing at the same song through every edit.
///
/// The current song is tracked by id: its position is refreshed, and the next
/// song recomputed. If it was removed from the queue and the player is
/// stopped, `current` is forgotten (MPD `DeleteInternal`: "there's a
/// 'current song' but we're not playing currently - clear 'current'"). A
/// removed song that is still playing/paused is left for the delete handler,
/// which has to decide what plays instead.
pub(crate) async fn sync_current_with_queue(state: &AppState) {
    let mut status = state.status.write().await;
    let Some(current) = status.current_song else {
        return;
    };
    let queue = state.queue.read().await;
    match queue.get_by_id(current.id) {
        Some(item) => {
            status.current_song = Some(QueuePosition {
                position: item.position,
                id: current.id,
            });
            update_next_song(&mut status, &queue, item.position);
        }
        None => {
            let player_state = PlayerState::from_atomic(state.atomic_state.load(Ordering::Acquire));
            if player_state == PlayerState::Stop {
                status.current_song = None;
                status.next_song = None;
            }
        }
    }
}

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
                        // A `SongFinished` that reaches us after the user stopped
                        // playback is stale (the decode thread ended naturally just
                        // before `stop`). The stopped player keeps its current song,
                        // so advancing here would restart playback behind the
                        // user's back.
                        if PlayerState::from_atomic(state.atomic_state.load(Ordering::Acquire))
                            == PlayerState::Stop
                        {
                            debug!("ignoring song-finished event: player already stopped");
                            continue;
                        }
                        info!("song finished, advancing to next");
                        if let Err(e) = Self::handle_song_finished(&state).await {
                            error!("error advancing to next song: {}", e);
                        }
                    }
                    Ok(Event::PlaybackError {
                        message,
                        output,
                        generation,
                    }) => {
                        warn!("playback error: {message}");
                        if let Err(e) =
                            Self::handle_playback_error(&state, message, output, generation).await
                        {
                            error!("error handling playback error: {}", e);
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
        // The song played through: that ends any streak of failing songs
        // (MPD `ResumePlayback`: no error => `error_count = 0`).
        state.playback_error_count.store(0, Ordering::Release);
        Self::advance(state, None).await
    }

    /// Handle a playback failure reported by the engine.
    ///
    /// Records `error:` for `status`, then reacts like MPD's
    /// `playlist::ResumePlayback`: an output error, a failure after a `seek`
    /// (`stop_on_error`), or as many failures in a row as the queue has
    /// songs stops playback — keeping the current song and the error — while
    /// any other decoder error skips on to the next song.
    ///
    /// `generation` is the engine's playback generation when the failing song
    /// was started. If the engine has moved on since (the user stopped,
    /// skipped or replayed in the meantime) the report is stale and is
    /// dropped: it must neither set `error:` nor stop/advance the song that
    /// is playing now.
    async fn handle_playback_error(
        state: &AppState,
        message: String,
        output: bool,
        generation: u64,
    ) -> rmpd_core::error::Result<()> {
        if state.engine.read().await.generation() != generation {
            debug!("ignoring stale playback error: {message}");
            return Ok(());
        }
        state.status.write().await.error = Some(message);

        let failures = state.playback_error_count.fetch_add(1, Ordering::AcqRel) + 1;
        let queue_len = state.queue.read().await.len() as u32;
        if output || state.stop_on_error.load(Ordering::Acquire) || failures >= queue_len {
            debug!("too many playback errors or critical error: stopping playback");
            stop_playback_guarded(state, false, Some(generation)).await?;
            return Ok(());
        }
        Self::advance(state, Some(generation)).await
    }

    /// Move on after the current song ended (finished or failed): play the
    /// next song, or stop at the end of the queue.
    ///
    /// `expected` is the engine generation the caller is reacting to (a
    /// failure report); every engine-mutating step re-checks it under the
    /// engine lock and gives up if a client got in first.
    async fn advance(state: &AppState, expected: Option<u64>) -> rmpd_core::error::Result<()> {
        // MPD `PlayNext`: moving on is not a seek, so a failure of the next
        // song skips again instead of stopping.
        state.stop_on_error.store(false, Ordering::Release);
        let (current, repeat, random, single, consume) = {
            let status = state.status.read().await;
            (
                status.current_song,
                status.repeat,
                status.random,
                status.single,
                status.consume,
            )
        };
        let Some(current) = current else {
            // Playback ended with no song selected (e.g. the playing song was
            // removed from the queue): don't sit in "play" with a dead decode
            // thread.
            stop_playback_guarded(state, true, expected).await?;
            return Ok(());
        };

        let next = {
            let queue = state.queue.read().await;
            next_position(
                &queue,
                current.position,
                repeat,
                random,
                consume != ConsumeMode::Off,
            )
        };

        let Some(next_pos) = next else {
            // End of queue (no repeat) or nothing left to play: stop and
            // clear the current song (MPD `PlayNext`: `Stop(); current = -1`),
            // then consume the song that just finished.
            debug!("no next song to play, stopping playback");
            if stop_playback_guarded(state, true, expected).await? {
                consume_finished(state, current).await;
            }
            return Ok(());
        };

        // Get the next song
        let target = {
            let queue = state.queue.read().await;
            queue
                .get(next_pos)
                .map(|item| ((*item.song).clone(), item.id, item.range))
        };
        let Some((song, item_id, range)) = target else {
            debug!("resolved next position vanished, stopping playback");
            stop_playback_guarded(state, true, expected).await?;
            return Ok(());
        };

        let playback_song = match prepare_song_for_playback(
            &song,
            state.music_dir.as_deref(),
            range,
            &state.sources,
        )
        .await
        {
            Ok(ps) => ps,
            Err(e) => {
                error!("failed to resolve next song: {}", e);
                // Treat it as that song failing to play: select it, report the
                // error, and let the failure handling above skip on or stop.
                {
                    let mut status = state.status.write().await;
                    status.current_song = Some(QueuePosition {
                        position: next_pos,
                        id: item_id,
                    });
                    status.next_song = None;
                }
                let generation = state.engine.read().await.generation();
                state.event_bus.emit(Event::PlaybackError {
                    message: format!("Failed to decode {:?}: {e}", song.path.as_str()),
                    output: false,
                    generation,
                });
                return Ok(());
            }
        };

        let played = {
            let mut engine = state.engine.write().await;
            if expected.is_some_and(|g| engine.generation() != g) {
                debug!("playback moved on while advancing; dropping stale advance");
                return Ok(());
            }
            engine.play(playback_song).await
        };
        match played {
            Ok(_) => {
                {
                    let mut status = state.status.write().await;
                    status.state = PlayerState::Play;
                    status.elapsed = Some(Duration::ZERO);
                    status.duration = song.duration;
                    status.bitrate = song.bitrate;
                    status.audio_format = helpers::extract_audio_format(&song);
                    // Starting a song clears any previous error (MPD
                    // `PlayerControl::SeekLocked` -> `ClearError`).
                    status.error = None;
                    status.current_song = Some(QueuePosition {
                        position: next_pos,
                        id: item_id,
                    });
                }

                // Handle consume mode (remove the finished song). Done after
                // the new song is current: the removal re-anchors positions
                // by id.
                consume_finished(state, current).await;

                // Handle single mode
                let should_stop_after = single == rmpd_core::state::SingleMode::On;
                if single == rmpd_core::state::SingleMode::Oneshot {
                    // Single oneshot: play one more song then stop
                    state.status.write().await.single = rmpd_core::state::SingleMode::Off;
                }

                // Refresh nextsong/nextsongid for the new current song (when
                // consume did not already do it through the queue version bump).
                sync_current_with_queue(state).await;

                state
                    .event_bus
                    .emit(Event::PlayerStateChanged(PlayerState::Play));
                state.event_bus.emit(Event::SongChanged(Some(song)));

                if should_stop_after {
                    stop_playback(state, false).await?;
                }
            }
            Err(e) => {
                error!("failed to play next song: {}", e);
            }
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
        consume: bool,
    ) -> Option<u32> {
        if random || single.is_on() || single.is_oneshot() || queue_len == 0 {
            return None;
        }
        let next = current_pos + 1;
        if next < queue_len {
            Some(next)
        } else if repeat && (current_pos > 0 || !consume) {
            // With consume, MPD only wraps when the song being consumed is
            // not the one it would wrap onto (see `next_position`).
            Some(0)
        } else {
            None
        }
    }

    /// Feed the engine the upcoming song for gapless/crossfade look-ahead.
    pub async fn feed_next_song(state: &AppState) {
        // A stopped player keeps its current song, but has nothing to look
        // ahead from.
        if PlayerState::from_atomic(state.atomic_state.load(Ordering::Acquire)) == PlayerState::Stop
        {
            state.engine.read().await.set_next_song(None);
            return;
        }
        let (current_pos, repeat, random, single, consume) = {
            let status = state.status.read().await;
            match &status.current_song {
                Some(p) => (
                    p.position,
                    status.repeat,
                    status.random,
                    status.single,
                    status.consume != ConsumeMode::Off,
                ),
                None => {
                    drop(status);
                    state.engine.read().await.set_next_song(None);
                    return;
                }
            }
        };
        let next_ps = {
            let queue = state.queue.read().await;
            match Self::lookahead_next_pos(
                current_pos,
                queue.len() as u32,
                repeat,
                random,
                single,
                consume,
            ) {
                // Range-restricted songs (CUE virtual tracks / rangeid) are not
                // eligible for the in-thread gapless/crossfade look-ahead, which
                // doesn't seek/limit. They fall back to the SongFinished path,
                // where play() honors the range.
                Some(np) => match queue.get(np).filter(|item| item.range.is_none()) {
                    Some(item) => {
                        match prepare_song_for_playback(
                            &(*item.song).clone(),
                            state.music_dir.as_deref(),
                            item.range,
                            &state.sources,
                        )
                        .await
                        {
                            Ok(ps) => Some(ps),
                            Err(e) => {
                                tracing::warn!("failed to resolve look-ahead song: {}", e);
                                None
                            }
                        }
                    }
                    None => None,
                },
                None => None,
            }
        };
        state.engine.read().await.set_next_song(next_ps);
    }

    /// Handle in-thread advance event — the engine already started the next song
    /// gaplessly/via crossfade; we only update bookkeeping (no engine.play call).
    async fn handle_advanced(state: &AppState) -> rmpd_core::error::Result<()> {
        let (current, repeat, random, single, consume) = {
            let s = state.status.read().await;
            match s.current_song {
                Some(p) => (p, s.repeat, s.random, s.single, s.consume),
                None => return Ok(()),
            }
        };
        let next_pos = match Self::lookahead_next_pos(
            current.position,
            state.queue.read().await.len() as u32,
            repeat,
            random,
            single,
            consume != ConsumeMode::Off,
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
        {
            let mut status = state.status.write().await;
            status.state = PlayerState::Play;
            status.elapsed = Some(Duration::ZERO);
            status.duration = song.duration;
            status.bitrate = song.bitrate;
            status.audio_format = helpers::extract_audio_format(&song);
            status.current_song = Some(QueuePosition {
                position: next_pos,
                id: item_id,
            });
            if single.is_oneshot() {
                status.single = rmpd_core::state::SingleMode::Off;
            }
        }
        // Consume the finished song (re-anchors the new current song's
        // position and next song through the queue version bump).
        consume_finished(state, current).await;
        sync_current_with_queue(state).await;
        state.event_bus.emit(Event::SongChanged(Some(song)));
        Ok(())
    }
}

impl Drop for QueuePlaybackManager {
    fn drop(&mut self) {
        self.stop();
    }
}

// Helper trait for SingleMode
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::state::SingleMode;
    use rmpd_core::test_utils::create_test_song;

    fn queue_of(n: u32) -> Queue {
        let mut queue = Queue::new();
        for i in 0..n {
            queue.add(create_test_song(u64::from(i), &i.to_string()));
        }
        queue
    }

    // ── next_position (MPD Queue::GetNextOrder) ─────────────────────────────

    #[test]
    fn next_position_sequential_advances_and_stops_at_the_end() {
        let queue = queue_of(3);
        assert_eq!(next_position(&queue, 0, false, false, false), Some(1));
        assert_eq!(next_position(&queue, 1, false, false, false), Some(2));
        assert_eq!(next_position(&queue, 2, false, false, false), None);
    }

    #[test]
    fn next_position_repeat_wraps_to_the_first_song() {
        let queue = queue_of(3);
        assert_eq!(next_position(&queue, 2, true, false, false), Some(0));
        // A single-song queue repeats itself when nothing is consumed…
        let one = queue_of(1);
        assert_eq!(next_position(&one, 0, true, false, false), Some(0));
    }

    #[test]
    fn next_position_repeat_with_consume_does_not_wrap_onto_the_consumed_song() {
        // GetNextOrder: `repeat && (_order > 0 || consume == OFF)`.
        let queue = queue_of(3);
        assert_eq!(next_position(&queue, 2, true, false, true), Some(0));
        // …but with consume the lone song is being removed: end of queue.
        let one = queue_of(1);
        assert_eq!(next_position(&one, 0, true, false, true), None);
        // The wrap rule only concerns the song at order 0.
        let two = queue_of(2);
        assert_eq!(next_position(&two, 0, true, false, true), Some(1));
    }

    #[test]
    fn next_position_random_never_picks_the_finished_song_when_others_exist() {
        let queue = queue_of(4);
        for _ in 0..50 {
            let pos = next_position(&queue, 2, true, true, true).expect("a song");
            assert_ne!(pos, 2);
        }
    }

    #[test]
    fn next_position_random_last_song_follows_repeat_and_consume() {
        let one = queue_of(1);
        assert_eq!(next_position(&one, 0, true, true, false), Some(0));
        assert_eq!(next_position(&one, 0, false, true, false), None);
        // random+repeat+consume: the only song left is consumed, not replayed.
        assert_eq!(next_position(&one, 0, true, true, true), None);
    }

    #[test]
    fn lookahead_follows_the_same_consume_wrap_rule() {
        let off = SingleMode::Off;
        // Last of three, repeat on: wrap in both modes.
        assert_eq!(
            QueuePlaybackManager::lookahead_next_pos(2, 3, true, false, off, true),
            Some(0)
        );
        // Lone song under repeat+consume: nothing to look ahead to.
        assert_eq!(
            QueuePlaybackManager::lookahead_next_pos(0, 1, true, false, off, true),
            None
        );
        assert_eq!(
            QueuePlaybackManager::lookahead_next_pos(0, 1, true, false, off, false),
            Some(0)
        );
    }

    // ── sync_current_with_queue ─────────────────────────────────────────────

    fn state_with_queue(n: u32) -> AppState {
        let state = AppState::new();
        *state.queue.try_write().unwrap() = queue_of(n);
        state
    }

    #[tokio::test]
    async fn sync_follows_the_current_song_when_songs_are_inserted_before_it() {
        let state = state_with_queue(3);
        // Stopped on song id 2 (position 1).
        state.status.write().await.current_song = Some(QueuePosition { position: 1, id: 2 });

        state
            .queue
            .write()
            .await
            .add_at(create_test_song(9, "inserted"), Some(0));
        sync_current_with_queue(&state).await;

        let status = state.status.read().await;
        let current = status.current_song.unwrap();
        assert_eq!((current.position, current.id), (2, 2));
        let next = status.next_song.expect("the song after it");
        assert_eq!(next.position, 3);
    }

    #[tokio::test]
    async fn sync_forgets_a_removed_current_song_only_while_stopped() {
        let state = state_with_queue(3);
        state.status.write().await.current_song = Some(QueuePosition { position: 1, id: 2 });
        state.queue.write().await.delete_id(2);

        // Playing: the delete handler decides what plays instead.
        state
            .atomic_state
            .store(PlayerState::Play as u8, Ordering::Release);
        sync_current_with_queue(&state).await;
        assert!(state.status.read().await.current_song.is_some());

        // Stopped: MPD just clears `current`.
        state
            .atomic_state
            .store(PlayerState::Stop as u8, Ordering::Release);
        sync_current_with_queue(&state).await;
        let status = state.status.read().await;
        assert!(status.current_song.is_none());
        assert!(status.next_song.is_none());
    }

    // ── consume ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn consume_oneshot_removes_the_finished_song_and_turns_itself_off() {
        let state = state_with_queue(3);
        state.status.write().await.consume = ConsumeMode::Oneshot;

        consume_finished(&state, QueuePosition { position: 0, id: 1 }).await;

        assert_eq!(state.queue.read().await.len(), 2);
        assert!(state.queue.read().await.get_by_id(1).is_none());
        assert_eq!(state.status.read().await.consume, ConsumeMode::Off);
    }

    #[tokio::test]
    async fn consume_off_leaves_the_queue_alone() {
        let state = state_with_queue(2);
        consume_finished(&state, QueuePosition { position: 0, id: 1 }).await;
        assert_eq!(state.queue.read().await.len(), 2);
    }

    /// MPD 0.24.16 "fix consuming the wrong song after reshuffle in
    /// random+repeat mode", transposed to rmpd's order-less random mode: at
    /// every `SongFinished` the song that finished is the one consumed, the
    /// song chosen to follow is another one that is still in the queue at the
    /// recorded position, and when only the last song remains it is NOT
    /// replayed (and half-deleted) under repeat — playback ends, queue empty.
    #[tokio::test]
    async fn random_repeat_consume_always_consumes_the_finished_song() {
        let state = AppState::new();
        // Random picks differ run to run: repeat the scenario to cover them.
        for _ in 0..20 {
            *state.queue.write().await = queue_of(4);
            {
                let mut status = state.status.write().await;
                status.random = true;
                status.repeat = true;
                status.consume = ConsumeMode::On;
                status.current_song = Some(QueuePosition { position: 0, id: 1 });
            }

            for round in 0..4usize {
                let finished = state.status.read().await.current_song.unwrap();
                QueuePlaybackManager::handle_song_finished(&state)
                    .await
                    .unwrap();

                let queue = state.queue.read().await;
                let status = state.status.read().await;
                assert!(
                    queue.get_by_id(finished.id).is_none(),
                    "round {round}: the finished song {} must be consumed",
                    finished.id
                );
                assert_eq!(queue.len(), 3 - round.min(3));
                if round < 3 {
                    let current = status.current_song.expect("a song to play next");
                    assert_ne!(current.id, finished.id, "round {round}");
                    assert_eq!(
                        queue.get_by_id(current.id).map(|i| i.position),
                        Some(current.position),
                        "round {round}: current must point at its queue item"
                    );
                } else {
                    assert!(queue.is_empty(), "last song consumed");
                    assert!(status.current_song.is_none(), "end of queue clears current");
                    assert_eq!(state.atomic_state.load(Ordering::Acquire), 0, "stopped");
                }
            }
        }
    }

    #[tokio::test]
    async fn sequential_repeat_consume_of_a_lone_song_ends_playback() {
        let state = state_with_queue(1);
        {
            let mut status = state.status.write().await;
            status.repeat = true;
            status.consume = ConsumeMode::On;
            status.current_song = Some(QueuePosition { position: 0, id: 1 });
        }
        QueuePlaybackManager::handle_song_finished(&state)
            .await
            .unwrap();
        assert!(state.queue.read().await.is_empty());
        assert!(state.status.read().await.current_song.is_none());
    }

    // ── error handling ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn output_error_stops_but_keeps_the_current_song_and_the_message() {
        let state = state_with_queue(3);
        state.status.write().await.current_song = Some(QueuePosition { position: 1, id: 2 });

        let generation = state.engine.read().await.generation();
        QueuePlaybackManager::handle_playback_error(
            &state,
            "no device".to_owned(),
            true,
            generation,
        )
        .await
        .unwrap();

        let status = state.status.read().await;
        assert_eq!(status.error.as_deref(), Some("no device"));
        assert_eq!(status.state, PlayerState::Stop);
        assert_eq!(status.current_song.map(|c| c.id), Some(2));
        assert_eq!(
            state.queue.read().await.len(),
            3,
            "nothing consumed/skipped"
        );
    }

    #[tokio::test]
    async fn decoder_errors_stop_once_the_queue_length_is_reached() {
        let state = state_with_queue(2);
        state.status.write().await.current_song = Some(QueuePosition { position: 0, id: 1 });

        // First failure: below the budget -> carries on with the next song
        // (the engine starts it; its own decode failure would come later).
        let generation = state.engine.read().await.generation();
        QueuePlaybackManager::handle_playback_error(&state, "first".to_owned(), false, generation)
            .await
            .unwrap();
        assert_eq!(
            state.status.read().await.current_song.map(|c| c.id),
            Some(2)
        );
        assert_eq!(state.playback_error_count.load(Ordering::Acquire), 1);

        // Second failure in a row == queue length: give up, keep the song.
        // Skipping started the next song, i.e. a new playback generation.
        let generation = state.engine.read().await.generation();
        QueuePlaybackManager::handle_playback_error(&state, "second".to_owned(), false, generation)
            .await
            .unwrap();
        let status = state.status.read().await;
        assert_eq!(status.error.as_deref(), Some("second"));
        assert_eq!(status.state, PlayerState::Stop);
        assert_eq!(status.current_song.map(|c| c.id), Some(2));
    }

    #[tokio::test]
    async fn stop_on_error_stops_instead_of_skipping() {
        let state = state_with_queue(3);
        state.status.write().await.current_song = Some(QueuePosition { position: 0, id: 1 });
        state.begin_playback_attempt(true).await;

        let generation = state.engine.read().await.generation();
        QueuePlaybackManager::handle_playback_error(&state, "broken".to_owned(), false, generation)
            .await
            .unwrap();
        let status = state.status.read().await;
        assert_eq!(status.state, PlayerState::Stop);
        assert_eq!(status.current_song.map(|c| c.id), Some(1), "no skip");
    }
    /// A failure report from a song the user has since stopped or replaced
    /// (different engine generation) must be dropped: it neither sets
    /// `error:` nor stops/advances what is playing now.
    #[tokio::test]
    async fn stale_playback_error_is_ignored() {
        let state = state_with_queue(3);
        state.status.write().await.current_song = Some(QueuePosition { position: 0, id: 1 });
        state.status.write().await.state = PlayerState::Play;
        let current = state.engine.read().await.generation();

        for stale in [current.wrapping_add(1), current.wrapping_sub(1)] {
            QueuePlaybackManager::handle_playback_error(
                &state,
                "from an aborted song".to_owned(),
                false,
                stale,
            )
            .await
            .unwrap();
        }
        // An output error would stop playback if it were taken seriously.
        QueuePlaybackManager::handle_playback_error(
            &state,
            "dead output".to_owned(),
            true,
            current.wrapping_add(7),
        )
        .await
        .unwrap();

        let status = state.status.read().await;
        assert_eq!(status.error, None, "stale reports leave no error");
        assert_eq!(status.state, PlayerState::Play, "nothing was stopped");
        assert_eq!(
            status.current_song.map(|c| c.id),
            Some(1),
            "nothing advanced"
        );
        assert_eq!(state.playback_error_count.load(Ordering::Acquire), 0);
    }

    /// `stop_playback_guarded` re-checks the generation under the engine
    /// lock: a mismatch changes nothing, a match stops.
    #[tokio::test]
    async fn guarded_stop_only_acts_on_the_expected_generation() {
        let state = state_with_queue(2);
        state.status.write().await.state = PlayerState::Play;
        state.status.write().await.current_song = Some(QueuePosition { position: 0, id: 1 });
        let generation = state.engine.read().await.generation();

        assert!(
            !stop_playback_guarded(&state, true, Some(generation.wrapping_add(1)))
                .await
                .unwrap()
        );
        assert_eq!(state.status.read().await.state, PlayerState::Play);
        assert!(state.status.read().await.current_song.is_some());

        assert!(
            stop_playback_guarded(&state, true, Some(generation))
                .await
                .unwrap()
        );
        assert_eq!(state.status.read().await.state, PlayerState::Stop);
        assert!(state.status.read().await.current_song.is_none());
    }
}
