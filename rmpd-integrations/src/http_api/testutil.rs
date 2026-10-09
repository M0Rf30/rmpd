// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared test fixtures: a recording [`PlayerHandle`] and a song builder.

use async_trait::async_trait;
use parking_lot::Mutex;
use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{
    BrowseEntry, BrowseKind, PlayerHandle, PlayerOptions, PlayerSnapshot, QueueEntry,
};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

/// Build a song with the given path and `(tag, value)` pairs.
pub(crate) fn song(path: &str, tags: &[(&'static str, &str)]) -> Song {
    Song {
        id: 1,
        path: path.into(),
        duration: Some(Duration::from_millis(183_500)),
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: Some(320),
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        range: None,
        tags: tags
            .iter()
            .map(|(k, v)| (Cow::Borrowed(*k), (*v).to_owned()))
            .collect(),
    }
}

/// Player double that records every call as a string.
#[derive(Default)]
pub(crate) struct MockPlayer {
    pub calls: Mutex<Vec<String>>,
    pub state: Mutex<PlayerState>,
    pub volume: Mutex<u8>,
    pub queue: Mutex<Vec<QueueEntry>>,
    pub opts: Mutex<PlayerOptions>,
}

impl MockPlayer {
    /// A player with a two-song queue (ids 10 and 11).
    pub(crate) fn with_queue() -> Self {
        let player = Self::default();
        *player.volume.lock() = 40;
        *player.queue.lock() = vec![
            QueueEntry {
                id: 10,
                position: 0,
                song: Arc::new(song("a.flac", &[("title", "First"), ("artist", "Me")])),
            },
            QueueEntry {
                id: 11,
                position: 1,
                song: Arc::new(song("b.flac", &[("title", "Second")])),
            },
        ];
        player
    }

    fn log(&self, call: impl Into<String>) {
        self.calls.lock().push(call.into());
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }
}

#[async_trait]
impl PlayerHandle for MockPlayer {
    async fn status(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            state: *self.state.lock(),
            elapsed: Some(Duration::from_millis(1500)),
            duration: Some(Duration::from_secs(180)),
            volume: *self.volume.lock(),
            song: self.queue.lock().first().map(|e| e.song.clone()),
        }
    }
    async fn play(&self) -> Result<(), PluginError> {
        self.log("play");
        *self.state.lock() = PlayerState::Play;
        Ok(())
    }
    async fn pause(&self) -> Result<(), PluginError> {
        self.log("pause");
        *self.state.lock() = PlayerState::Pause;
        Ok(())
    }
    async fn toggle(&self) -> Result<(), PluginError> {
        self.log("toggle");
        *self.state.lock() = PlayerState::Play;
        Ok(())
    }
    async fn next(&self) -> Result<(), PluginError> {
        self.log("next");
        Ok(())
    }
    async fn previous(&self) -> Result<(), PluginError> {
        self.log("previous");
        Ok(())
    }
    async fn stop(&self) -> Result<(), PluginError> {
        self.log("stop");
        *self.state.lock() = PlayerState::Stop;
        Ok(())
    }
    async fn set_volume(&self, volume: u8) -> Result<(), PluginError> {
        self.log(format!("set_volume:{volume}"));
        *self.volume.lock() = volume;
        Ok(())
    }
    async fn seek(&self, position: Duration) -> Result<(), PluginError> {
        self.log(format!("seek:{}", position.as_millis()));
        Ok(())
    }
    async fn current_song_id(&self) -> Option<u32> {
        self.queue.lock().first().map(|e| e.id)
    }
    async fn queue_len(&self) -> usize {
        self.queue.lock().len()
    }
    async fn options(&self) -> PlayerOptions {
        *self.opts.lock()
    }
    async fn set_repeat(&self, on: bool) -> Result<(), PluginError> {
        self.log(format!("set_repeat:{on}"));
        self.opts.lock().repeat = on;
        Ok(())
    }
    async fn set_random(&self, on: bool) -> Result<(), PluginError> {
        self.log(format!("set_random:{on}"));
        self.opts.lock().random = on;
        Ok(())
    }
    async fn set_single(&self, on: bool) -> Result<(), PluginError> {
        self.log(format!("set_single:{on}"));
        self.opts.lock().single = on;
        Ok(())
    }
    async fn position(&self) -> Option<Duration> {
        Some(Duration::from_millis(2500))
    }
    async fn queue_entries(&self) -> Vec<QueueEntry> {
        self.queue.lock().clone()
    }
    async fn add_uris(
        &self,
        uris: &[String],
        position: Option<u32>,
    ) -> Result<Vec<QueueEntry>, PluginError> {
        self.log(format!("add_uris:{}:{position:?}", uris.join(",")));
        Ok(uris
            .iter()
            .zip(100u32..)
            .map(|(uri, id)| QueueEntry {
                id,
                position: id - 100,
                song: Arc::new(song(uri, &[])),
            })
            .collect())
    }
    async fn clear_queue(&self) -> Result<(), PluginError> {
        self.log("clear");
        Ok(())
    }
    async fn play_id(&self, id: u32) -> Result<(), PluginError> {
        self.log(format!("play_id:{id}"));
        Ok(())
    }
    async fn browse(&self, uri: Option<&str>) -> Result<Vec<BrowseEntry>, PluginError> {
        self.log(format!("browse:{uri:?}"));
        Ok(vec![
            BrowseEntry {
                kind: BrowseKind::Directory,
                uri: "dir".to_owned(),
                name: "dir".to_owned(),
            },
            BrowseEntry {
                kind: BrowseKind::Track,
                uri: "a.flac".to_owned(),
                name: "First".to_owned(),
            },
        ])
    }
    async fn search(&self, query: &str) -> Result<Vec<Song>, PluginError> {
        self.log(format!("search:{query}"));
        Ok(vec![song("found.flac", &[("title", "Found")])])
    }
}
