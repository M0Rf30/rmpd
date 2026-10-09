// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`PlayerHandle`] implementation over the live server [`AppState`], handed
//! to integrations (see `rmpd_plugin::integration`).
//!
//! Controls go through the same command handlers MPD clients hit, so
//! idle events, queue semantics and error handling are identical.

use crate::commands::utils::open_db;
use crate::commands::{options, playback, queue};
use crate::helpers;
use crate::parser::InsertPosition;
use crate::state::AppState;
use async_trait::async_trait;
use rmpd_core::history::HistoryEntry;
use rmpd_core::song::Song;
use rmpd_core::state::{PlayerState, SingleMode};
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::{
    BrowseEntry, BrowseKind, PlayerHandle, PlayerOptions, PlayerSnapshot, QueueEntry,
};
use std::collections::HashSet;
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

/// Search results are capped so a one-letter query cannot flood a client.
const MAX_SEARCH_RESULTS: usize = 500;

/// Path relative to the music directory (database paths may be absolute).
fn display_path(path: &str, music_dir: Option<&str>) -> String {
    music_dir
        .and_then(|dir| path.strip_prefix(dir))
        .map_or(path, |rest| rest.trim_start_matches('/'))
        .to_owned()
}

/// Last `/`-separated segment (the whole string when it has no `/`).
fn last_segment(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_owned()
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
        // Effective mixer volume (hardware mixers can be moved externally).
        let effective_volume = options::current_volume(&self.state)
            .await
            .unwrap_or(status.volume);
        PlayerSnapshot {
            state: status.state,
            elapsed: status.elapsed,
            duration: status.duration,
            volume: effective_volume,
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

    async fn queue_entries(&self) -> Vec<QueueEntry> {
        self.state
            .queue
            .read()
            .await
            .items()
            .iter()
            .map(|item| QueueEntry {
                id: item.id,
                position: item.position,
                song: item.song.clone(),
            })
            .collect()
    }

    async fn add_uris(
        &self,
        uris: &[String],
        position: Option<u32>,
    ) -> Result<Vec<QueueEntry>, PluginError> {
        let before: HashSet<u32> = self.queue_entries().await.iter().map(|e| e.id).collect();
        let mut at = position;
        for uri in uris {
            let insert = at.map(InsertPosition::Absolute);
            let resp = queue::handle_addid_command(&self.state, uri, insert).await;
            if resp.starts_with("ACK") {
                // Not a single song (e.g. a directory): `add` resolves those.
                check(&queue::handle_add_command(&self.state, uri, insert).await)?;
            }
            if let Some(start) = position {
                let added = self
                    .queue_entries()
                    .await
                    .iter()
                    .filter(|e| !before.contains(&e.id))
                    .count();
                at = Some(start.saturating_add(u32::try_from(added).unwrap_or(u32::MAX)));
            }
        }
        Ok(self
            .queue_entries()
            .await
            .into_iter()
            .filter(|e| !before.contains(&e.id))
            .collect())
    }

    async fn clear_queue(&self) -> Result<(), PluginError> {
        check(&queue::handle_clear_command(&self.state).await)
    }

    async fn play_id(&self, id: u32) -> Result<(), PluginError> {
        check(&queue::handle_playid_command(&self.state, Some(id)).await)
    }

    async fn browse(&self, uri: Option<&str>) -> Result<Vec<BrowseEntry>, PluginError> {
        let path = uri.unwrap_or("").trim_matches('/').to_owned();
        let music_dir = self.state.music_dir.clone();

        // Under an on-demand source mount the source itself answers.
        if !path.is_empty()
            && self
                .state
                .sources
                .owning_source(&path)
                .is_some_and(|s| s.sync_policy() == rmpd_source::SyncPolicy::OnDemand)
        {
            let sources = self.state.sources.clone();
            let p = path.clone();
            // Spawned so the (non-Sync) async_trait future of the source does
            // not leak into this future's bounds.
            let result = tokio::spawn(async move { sources.browse_on_demand(&p).await })
                .await
                .map_err(|e| PluginError::Runtime(e.to_string()))?;
            return match result {
                Some(Ok(entries)) => Ok(entries
                    .into_iter()
                    .map(|entry| match entry {
                        rmpd_source::SourceEntry::Song(song) => BrowseEntry {
                            kind: BrowseKind::Track,
                            uri: song.path.to_string(),
                            name: song.display_title().to_owned(),
                        },
                        rmpd_source::SourceEntry::Dir(dir) => BrowseEntry {
                            kind: BrowseKind::Directory,
                            name: last_segment(&dir),
                            uri: dir,
                        },
                    })
                    .collect()),
                Some(Err(e)) => Err(PluginError::Runtime(e.to_string())),
                None => Err(PluginError::Runtime("No such directory".to_owned())),
            };
        }

        let state = self.state.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<BrowseEntry>, PluginError> {
            let db =
                open_db(&state, "browse").map_err(|e| PluginError::Runtime(e.trim().to_owned()))?;
            let listing = db
                .list_directory(&path)
                .map_err(|e| PluginError::Runtime(e.to_string()))?;
            let mut out = Vec::new();
            for (dir, _mtime) in &listing.directories {
                let dir = display_path(dir, music_dir.as_deref());
                out.push(BrowseEntry {
                    kind: BrowseKind::Directory,
                    name: last_segment(&dir),
                    uri: dir,
                });
            }
            for song in &listing.songs {
                out.push(BrowseEntry {
                    kind: BrowseKind::Track,
                    uri: display_path(song.path.as_str(), music_dir.as_deref()),
                    name: song.display_title().to_owned(),
                });
            }
            // On-demand sources are never mirrored into the database, so
            // surface their mount points at the root explicitly.
            if path.is_empty() {
                for source in state.sources.iter() {
                    if source.sync_policy() == rmpd_source::SyncPolicy::OnDemand {
                        out.push(BrowseEntry {
                            kind: BrowseKind::Directory,
                            name: source.name().to_owned(),
                            uri: source.name().to_owned(),
                        });
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| PluginError::Runtime(e.to_string()))?
    }

    async fn search(&self, query: &str) -> Result<Vec<Song>, PluginError> {
        let state = self.state.clone();
        let filters = vec![("any".to_owned(), query.to_owned())];
        tokio::task::spawn_blocking(move || -> Result<Vec<Song>, PluginError> {
            let db =
                open_db(&state, "search").map_err(|e| PluginError::Runtime(e.trim().to_owned()))?;
            let mut songs = helpers::resolve_filters(&db, &filters, "search", false)
                .map_err(|e| PluginError::Runtime(e.trim().to_owned()))?;
            songs.truncate(MAX_SEARCH_RESULTS);
            for song in &mut songs {
                let shown = display_path(song.path.as_str(), state.music_dir.as_deref());
                song.path = shown.into();
            }
            Ok(songs)
        })
        .await
        .map_err(|e| PluginError::Runtime(e.to_string()))?
    }

    async fn history(&self) -> Vec<HistoryEntry> {
        self.state.history.newest_first()
    }

    async fn history_length(&self) -> usize {
        self.state.history.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_path_strips_music_dir() {
        assert_eq!(display_path("/m/a/b.flac", Some("/m")), "a/b.flac");
        assert_eq!(display_path("/m/a/b.flac", Some("/m/")), "a/b.flac");
        assert_eq!(display_path("a/b.flac", Some("/m")), "a/b.flac");
        assert_eq!(display_path("a/b.flac", None), "a/b.flac");
    }

    #[test]
    fn last_segment_handles_trailing_slash() {
        assert_eq!(last_segment("a/b/c"), "c");
        assert_eq!(last_segment("a/b/"), "b");
        assert_eq!(last_segment("root"), "root");
    }

    #[tokio::test]
    async fn history_is_served_newest_first() {
        let state = AppState::new();
        for (n, uri) in ["a.flac", "b.flac", "c.flac"].into_iter().enumerate() {
            state.history.push(HistoryEntry {
                timestamp_ms: n as u64,
                uri: uri.to_owned(),
                title: None,
                artist: None,
                album: None,
            });
        }
        let handle = ServerPlayerHandle::new(state);
        let uris: Vec<String> = handle.history().await.into_iter().map(|e| e.uri).collect();
        assert_eq!(uris, ["c.flac", "b.flac", "a.flac"]);
        assert_eq!(handle.history_length().await, 3);
    }
}
