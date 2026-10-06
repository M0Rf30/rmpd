// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Playback control command handlers

use std::sync::atomic::Ordering;
use std::time::Duration;

use rmpd_core::error::RmpdError;
use rmpd_core::event::Event;
use rmpd_core::state::{ConsumeMode, PlayerState, QueuePosition};
use tracing::{debug, error};

use crate::helpers;
use crate::queue_playback::{consume_finished, next_position, stop_playback};
use crate::response::ResponseBuilder;
use crate::state::AppState;

use super::utils::{
    ACK_ERROR_ARG, ACK_ERROR_NO_EXIST, ACK_ERROR_PLAYER_SYNC, ACK_ERROR_SYS, ACK_ERROR_UNKNOWN,
    prepare_song_for_playback, update_next_song,
};

/// The player's state, read lock-free from the engine's atomic.
///
/// MPD's `playlist::playing` flag — "the player is playing or paused" — is
/// what gates `next`, `previous` and `seekcur`. It is NOT the same as "there
/// is a current song": a stopped player keeps its current song.
fn player_state(state: &AppState) -> PlayerState {
    PlayerState::from_atomic(state.atomic_state.load(Ordering::Acquire))
}

fn not_playing(command: &str) -> String {
    ResponseBuilder::error(ACK_ERROR_PLAYER_SYNC, 0, command, "Not playing")
}

/// A queue item that was just handed to the engine.
pub(crate) struct StartedSong {
    pub song: rmpd_core::song::Song,
}

/// Start playing the queue item at `position` on the engine and record it as
/// the current song, without announcing it to idle clients yet (see
/// [`announce_started`]) so callers can still adjust the queue first.
///
/// Starting a song clears any previous playback error, like MPD's
/// `PlayerControl::SeekLocked` (`ClearError`).
pub(crate) async fn engine_play_item(
    state: &AppState,
    command: &str,
    position: u32,
) -> Result<StartedSong, String> {
    let (song, item_id, range) = {
        let queue = state.queue.read().await;
        match queue.get(position) {
            Some(item) => ((*item.song).clone(), item.id, item.range),
            None => {
                return Err(ResponseBuilder::error(
                    ACK_ERROR_ARG,
                    0,
                    command,
                    "Bad song index",
                ));
            }
        }
    };

    let playback_song =
        prepare_song_for_playback(&song, state.music_dir.as_deref(), range, &state.sources)
            .await
            .map_err(|e| {
                ResponseBuilder::error(
                    ACK_ERROR_NO_EXIST,
                    0,
                    command,
                    &format!("Cannot resolve song: {e}"),
                )
            })?;

    if let Err(e) = state.engine.write().await.play(playback_song).await {
        error!("{command} failed: {e}");
        return Err(ResponseBuilder::error(
            ACK_ERROR_SYS,
            0,
            command,
            &format!("Playback error: {e}"),
        ));
    }

    {
        let mut status = state.status.write().await;
        status.state = PlayerState::Play;
        status.elapsed = Some(Duration::ZERO);
        status.duration = song.duration;
        status.bitrate = song.bitrate;
        status.audio_format = helpers::extract_audio_format(&song);
        status.error = None;
        status.current_song = Some(QueuePosition {
            position,
            id: item_id,
        });
        let queue = state.queue.read().await;
        update_next_song(&mut status, &queue, position);
    }

    Ok(StartedSong { song })
}

/// Notify idle clients (`player` subsystem) that a new song started playing.
pub(crate) fn announce_started(state: &AppState, song: rmpd_core::song::Song) {
    debug!("emitting PlayerStateChanged(Play) and SongChanged events");
    state
        .event_bus
        .emit(Event::PlayerStateChanged(PlayerState::Play));
    state.event_bus.emit(Event::SongChanged(Some(song)));
}

/// `play POS` / `playid ID` once the target position is known: MPD
/// `playlist::PlayPosition`.
pub(crate) async fn play_position(state: &AppState, command: &str, position: u32) -> String {
    // PlayPosition clears the error and the failure streak before it even
    // validates the position.
    state.begin_playback_attempt(false).await;
    match engine_play_item(state, command, position).await {
        Ok(started) => {
            announce_started(state, started.song);
            ResponseBuilder::new().ok()
        }
        Err(resp) => resp,
    }
}

/// `play` / `playid` without an argument: MPD `playlist::PlayAny`.
///
/// - an empty queue is a silent no-op;
/// - while playing it does nothing, while paused it resumes;
/// - while stopped it (re)starts the song the player stopped on (the current
///   song survives `stop`), or else the first one — a random one in random
///   mode, since MPD's play order is the shuffled order there.
pub(crate) async fn play_any(state: &AppState, command: &str) -> String {
    if state.queue.read().await.is_empty() {
        return ResponseBuilder::new().ok();
    }
    state.status.write().await.error = None;
    match player_state(state) {
        PlayerState::Play => return ResponseBuilder::new().ok(),
        PlayerState::Pause => return handle_pause_command(state, Some(false)).await,
        PlayerState::Stop => {}
    }
    state.begin_playback_attempt(false).await;

    let (current, random) = {
        let status = state.status.read().await;
        (status.current_song, status.random)
    };
    let position = {
        let queue = state.queue.read().await;
        let len = queue.len() as u32;
        current
            .and_then(|c| {
                queue
                    .get_by_id(c.id)
                    .map(|item| item.position)
                    .or((c.position < len).then_some(c.position))
            })
            .or_else(|| {
                if random {
                    queue.weighted_random_pos(None)
                } else {
                    None
                }
            })
            .unwrap_or(0)
    };
    match engine_play_item(state, command, position).await {
        Ok(started) => {
            announce_started(state, started.song);
            ResponseBuilder::new().ok()
        }
        Err(resp) => resp,
    }
}

pub async fn handle_play_command(state: &AppState, position: Option<u32>) -> String {
    match position {
        Some(pos) => play_position(state, "play", pos).await,
        None => play_any(state, "play").await,
    }
}

pub async fn handle_pause_command(state: &AppState, pause_state: Option<bool>) -> String {
    // Get current state lock-free using atomic (no engine lock needed!)
    let current_state = player_state(state);

    let should_pause = pause_state.unwrap_or_else(|| current_state == PlayerState::Play);
    let is_currently_paused = current_state == PlayerState::Pause;

    // If already in desired state, do nothing
    if should_pause == is_currently_paused {
        return ResponseBuilder::new().ok();
    }

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
            let actual_state = player_state(state);

            debug!("emitting PlayerStateChanged({:?}) event", actual_state);
            helpers::update_player_state(state, actual_state).await;

            ResponseBuilder::new().ok()
        }
        Err(e) => {
            error!("pause failed: {}", e);
            ResponseBuilder::error(ACK_ERROR_SYS, 0, "pause", &format!("Pause error: {e}"))
        }
    }
}

pub async fn handle_stop_command(state: &AppState) -> String {
    // MPD `playlist::Stop` keeps the current song: `status` still reports
    // `song`/`songid`/`nextsong`, `currentsong` still prints it, and a bare
    // `play` restarts it.
    debug!("emitting PlayerStateChanged(Stop) event");
    match stop_playback(state, false).await {
        Ok(()) => ResponseBuilder::new().ok(),
        Err(e) => ResponseBuilder::error(ACK_ERROR_SYS, 0, "stop", &format!("Stop error: {e}")),
    }
}

/// The current song and the options `next`/`previous` depend on, or the
/// "Not playing" ACK when the player is stopped (MPD: `if (!playing) throw
/// NotPlaying()` — a stopped player still has a current song, so the song
/// alone does not tell).
async fn playing_current(
    state: &AppState,
    command: &str,
) -> Result<(QueuePosition, bool, bool, ConsumeMode), String> {
    let status = state.status.read().await;
    match status.current_song {
        Some(current) if player_state(state) != PlayerState::Stop => {
            Ok((current, status.repeat, status.random, status.consume))
        }
        _ => Err(not_playing(command)),
    }
}

pub async fn handle_next_command(state: &AppState) -> String {
    let (current, repeat, random, consume) = match playing_current(state, "next").await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // MPD `PlayNext`: `stop_on_error = false`.
    state.stop_on_error.store(false, Ordering::Release);

    let target_pos = {
        let queue = state.queue.read().await;
        next_position(
            &queue,
            current.position,
            repeat,
            random,
            consume != ConsumeMode::Off,
        )
    };

    let Some(target_pos) = target_pos else {
        // End of queue without repeat: MPD stops and replies OK, not an ACK
        // error; the current song is reset, and consume still removes the
        // song that was playing.
        return match stop_playback(state, true).await {
            Ok(()) => {
                consume_finished(state, current).await;
                ResponseBuilder::new().ok()
            }
            Err(e) => {
                ResponseBuilder::error(ACK_ERROR_SYS, 0, "next", &format!("Playback error: {e}"))
            }
        };
    };

    match engine_play_item(state, "next", target_pos).await {
        Ok(started) => {
            // Consume removes the song left behind; do it before announcing
            // so listeners already see the final queue.
            consume_finished(state, current).await;
            announce_started(state, started.song);
            ResponseBuilder::new().ok()
        }
        Err(resp) => resp,
    }
}

pub async fn handle_previous_command(state: &AppState) -> String {
    let (current, repeat, random, _consume) = match playing_current(state, "previous").await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let target_pos = {
        let queue = state.queue.read().await;
        let queue_len = queue.len() as u32;
        if random {
            match queue.weighted_random_pos(Some(current.position)) {
                Some(p) => p,
                None if repeat => queue.weighted_random_pos(None).unwrap_or(current.position),
                None => current.position,
            }
        } else if current.position > 0 {
            current.position - 1
        } else if repeat && queue_len > 0 {
            // Wrap to the last song when repeat is on (CMD-06).
            queue_len - 1
        } else {
            // Already at the first song, no repeat: MPD replays the same song.
            current.position
        }
    };

    // MPD's `playlist::PlayPrevious` just plays the target: unlike `next`, it
    // does not consume the song being left.
    match engine_play_item(state, "previous", target_pos).await {
        Ok(started) => {
            announce_started(state, started.song);
            ResponseBuilder::new().ok()
        }
        Err(resp) => resp,
    }
}

/// The ACK for a seek the engine could not carry out.
///
/// MPD 0.25 ("show detailed seek errors") no longer collapses a failed seek
/// into `Not playing`: `PlayerControl::SeekLocked` rethrows the player's
/// error, and `PrintError` (`src/command/CommandError.cxx`) answers with the
/// exception's full message — `Not seekable`, `Failed to decode "…": …` — under
/// `ACK_ERROR_UNKNOWN`, since none of those is a protocol/playlist error.
fn seek_error(command: &str, e: &RmpdError) -> String {
    match e {
        // The decode thread is gone: nothing is playing any more.
        RmpdError::InvalidState(_) => not_playing(command),
        RmpdError::Player(msg) => ResponseBuilder::error(ACK_ERROR_UNKNOWN, 0, command, msg),
        other => ResponseBuilder::error(ACK_ERROR_UNKNOWN, 0, command, &other.to_string()),
    }
}

/// Seek inside the song that is playing/paused right now and, on success,
/// record the position `status` should show until the next position event.
async fn seek_playing_song(state: &AppState, command: &str, time: f64) -> String {
    // Queue the seek under the engine lock, but wait for the decoder's verdict
    // (up to seconds, for a slow source) only after releasing it: holding it
    // would stall every other engine user — `status`, `stats`, `stop` — behind
    // this seek.
    let pending = state.engine.read().await.begin_seek(time);
    let result = match pending {
        Ok(pending) => pending.verdict().await,
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => {
            let mut status = state.status.write().await;
            let mut elapsed = Duration::try_from_secs_f64(time).unwrap_or(Duration::ZERO);
            if let Some(duration) = status.duration {
                // The engine clamps an out-of-range seek to the end of the song.
                elapsed = elapsed.min(duration);
            }
            status.elapsed = Some(elapsed);
            ResponseBuilder::new().ok()
        }
        Err(e) => seek_error(command, &e),
    }
}

/// MPD `playlist::SeekSongOrder`: seek within the song at `position` if it is
/// the one playing, otherwise start it and seek to `time`.
///
/// Both ways clear the previous error and arm `stop_on_error`, so a song that
/// fails to play after a seek stops playback instead of skipping ahead.
async fn seek_to_position(state: &AppState, command: &str, position: u32, time: f64) -> String {
    let is_playing_it = player_state(state) != PlayerState::Stop
        && state
            .status
            .read()
            .await
            .current_song
            .is_some_and(|c| c.position == position);

    state.begin_playback_attempt(true).await;
    if is_playing_it {
        return seek_playing_song(state, command, time).await;
    }

    match engine_play_item(state, command, position).await {
        Ok(started) => {
            announce_started(state, started.song);
            if time > 0.0 {
                seek_playing_song(state, command, time).await
            } else {
                ResponseBuilder::new().ok()
            }
        }
        Err(resp) => resp,
    }
}

pub async fn handle_seek_command(state: &AppState, position: u32, time: f64) -> String {
    // MPD SeekSongPosition: BadRange if the position is not in the queue.
    if state.queue.read().await.get(position).is_none() {
        return ResponseBuilder::error(ACK_ERROR_ARG, 0, "seek", "Bad song index");
    }
    seek_to_position(state, "seek", position, time).await
}

pub async fn handle_seekid_command(state: &AppState, id: u32, time: f64) -> String {
    // Find the song by ID first
    let position = match state.queue.read().await.get_by_id(id) {
        Some(item) => item.position,
        None => {
            return ResponseBuilder::error(ACK_ERROR_NO_EXIST, 0, "seekid", "No such song");
        }
    };
    seek_to_position(state, "seekid", position, time).await
}

pub async fn handle_seekcur_command(state: &AppState, time: f64, relative: bool) -> String {
    // MPD `playlist::SeekCurrent`: `if (!playing) throw NotPlaying`.
    if player_state(state) == PlayerState::Stop || state.status.read().await.current_song.is_none()
    {
        return not_playing("seekcur");
    }

    let seek_position = if relative {
        // Relative seek: add to the live position (like MPD, which reads the
        // player's current elapsed time), falling back to the last reported one.
        let elapsed = match state.engine.read().await.get_elapsed_live() {
            Some(live) => live,
            None => state.status.read().await.elapsed.unwrap_or(Duration::ZERO),
        };
        elapsed.as_secs_f64() + time
    } else {
        time
    };
    // A negative target is clamped to the start of the song.
    let seek_position = if seek_position.is_finite() {
        seek_position.max(0.0)
    } else {
        0.0
    };

    state.begin_playback_attempt(true).await;
    seek_playing_song(state, "seekcur", seek_position).await
}

/// The song the player was playing/paused on was just removed from the queue
/// (`delete`/`deleteid`), and `at` is the position that now holds what used to
/// follow it. Mirrors MPD's `playlist::DeleteInternal`:
///
/// - playing: carry on with the song that took its place (wrapping with
///   repeat), or stop when the queue ran out;
/// - paused: stop the player; `current` moves to the song that took its place.
///
/// A stopped player simply forgets its current song, which
/// [`crate::queue_playback::sync_current_with_queue`] already did.
pub(crate) async fn current_song_removed(state: &AppState, at: u32) {
    let Some(current) = state.status.read().await.current_song else {
        return;
    };
    let repeat = state.status.read().await.repeat;
    let (still_queued, len) = {
        let queue = state.queue.read().await;
        (queue.get_by_id(current.id).is_some(), queue.len() as u32)
    };
    let playing = player_state(state);
    if still_queued || playing == PlayerState::Stop {
        return;
    }

    let replacement = if at < len {
        Some(at)
    } else if repeat && len > 0 {
        Some(0)
    } else {
        None
    };

    match (playing, replacement) {
        (PlayerState::Play, Some(pos)) => match engine_play_item(state, "delete", pos).await {
            Ok(started) => announce_started(state, started.song),
            Err(_) => {
                let _ = stop_playback(state, true).await;
            }
        },
        (_, Some(pos)) => {
            // Paused: MPD stops the player but leaves `current` on the
            // replacement, so a following `play` starts it.
            let _ = stop_playback(state, false).await;
            // status before queue, the order every other path takes (the
            // reverse order can deadlock against a queued queue writer).
            let mut status = state.status.write().await;
            let queue = state.queue.read().await;
            status.current_song = queue.get(pos).map(|item| QueuePosition {
                position: pos,
                id: item.id,
            });
            update_next_song(&mut status, &queue, pos);
        }
        (_, None) => {
            let _ = stop_playback(state, true).await;
        }
    }
}
