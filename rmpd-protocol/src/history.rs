// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Play-history recorder: turns "a song started playing" events into
//! [`HistoryEntry`] values in [`AppState::history`].
//!
//! `Event::SongChanged(Some(song))` is emitted once the engine has actually
//! started a song (manual `play`/`playid`, queue advance and gapless
//! in-thread advance alike), so it is the right trigger. `SongChanged(None)`
//! (playback stopped) records nothing.

use crate::state::AppState;
use rmpd_core::event::Event;
use rmpd_core::history::{HistoryEntry, unix_now_ms};
use rmpd_core::song::Song;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// The same URI announced again within this window is one start reported
/// twice (e.g. an advance notification followed by the song-changed event),
/// not a second play.
const DUPLICATE_WINDOW: Duration = Duration::from_secs(1);

/// Decides which song-changed notifications are real, distinct plays.
#[derive(Debug, Default)]
pub struct Recorder {
    last: Option<(String, Instant)>,
}

impl Recorder {
    /// Entry to record for `song` starting now, or `None` for a duplicate
    /// notification of the song that just started.
    pub fn observe(&mut self, song: &Song, now: Instant, unix_ms: u64) -> Option<HistoryEntry> {
        let uri = song.path.as_str();
        if let Some((last_uri, at)) = &self.last
            && last_uri == uri
            && now.saturating_duration_since(*at) < DUPLICATE_WINDOW
        {
            return None;
        }
        self.last = Some((uri.to_owned(), now));
        Some(HistoryEntry::from_song(song, unix_ms))
    }
}

/// Start the background task recording plays into `state.history`.
///
/// Must be called from within a tokio runtime. The task ends when the event
/// bus closes.
pub fn spawn_recorder(state: &AppState) -> JoinHandle<()> {
    let history = state.history.clone();
    let mut rx = state.event_bus.subscribe();
    tokio::spawn(async move {
        let mut recorder = Recorder::default();
        loop {
            match rx.recv().await {
                Ok(Event::SongChanged(Some(song))) => {
                    if history.capacity() == 0 {
                        continue;
                    }
                    if let Some(entry) = recorder.observe(&song, Instant::now(), unix_now_ms()) {
                        debug!("history: recorded {}", entry.uri);
                        history.push(entry);
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => {
                    warn!("history recorder lagged, {n} events skipped");
                }
                Err(RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::test_utils::make_test_song;

    #[test]
    fn records_distinct_songs() {
        let mut r = Recorder::default();
        let t0 = Instant::now();
        let a = make_test_song("a.flac", 1);
        let b = make_test_song("b.flac", 2);
        assert_eq!(r.observe(&a, t0, 10).unwrap().uri, "a.flac");
        assert_eq!(r.observe(&b, t0, 11).unwrap().uri, "b.flac");
    }

    #[test]
    fn same_song_twice_in_a_burst_is_one_play() {
        let mut r = Recorder::default();
        let t0 = Instant::now();
        let a = make_test_song("a.flac", 1);
        assert!(r.observe(&a, t0, 10).is_some());
        assert!(r.observe(&a, t0 + Duration::from_millis(200), 11).is_none());
    }

    #[test]
    fn repeating_the_same_song_later_is_a_new_play() {
        let mut r = Recorder::default();
        let t0 = Instant::now();
        let a = make_test_song("a.flac", 1);
        assert!(r.observe(&a, t0, 10).is_some());
        assert!(r.observe(&a, t0 + Duration::from_secs(180), 190).is_some());
    }

    #[tokio::test]
    async fn recorder_task_records_started_songs_only() {
        let state = AppState::new();
        let _task = spawn_recorder(&state);

        state.event_bus.emit(Event::SongChanged(None));
        state
            .event_bus
            .emit(Event::SongChanged(Some(make_test_song("one.flac", 1))));
        state
            .event_bus
            .emit(Event::SongChanged(Some(make_test_song("two.flac", 2))));

        // Wait for the recorder task to drain the bus.
        for _ in 0..100 {
            if state.history.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let uris: Vec<String> = state
            .history
            .newest_first()
            .into_iter()
            .map(|e| e.uri)
            .collect();
        assert_eq!(uris, ["two.flac", "one.flac"]);
    }

    #[tokio::test]
    async fn disabled_history_records_nothing() {
        let mut state = AppState::new();
        state.set_history_length(0);
        let _task = spawn_recorder(&state);
        state
            .event_bus
            .emit(Event::SongChanged(Some(make_test_song("one.flac", 1))));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(state.history.is_empty());
    }
}
