// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regression: songs under an `OnDemand` source mount (radio, somafm, podcast)
//! are never in the database, so `add`/`addid` must resolve them through
//! `MusicSource::lookup` / `browse` and queue them with the mount-style path
//! (which `prepare_song_for_playback` later resolves via the source).

use async_trait::async_trait;
use rmpd_core::test_utils::make_test_song;
use rmpd_protocol::AppState;
use rmpd_protocol::commands::playlists::{handle_listplaylistinfo_command, handle_load_command};
use rmpd_protocol::commands::queue::{handle_add_command, handle_addid_command};
use rmpd_source::{
    MusicSource, SourceEntry, SourceError, SourceRegistry, SourceResult, SyncPolicy,
};
use std::sync::Arc;

/// Tree: root holds song `od/root` and dir `od/dir`; `od/dir` holds
/// `od/dir/one`. `lookup` returns songs with an unrelated internal path.
struct OnDemand;

#[async_trait]
impl MusicSource for OnDemand {
    fn scheme(&self) -> &str {
        "od"
    }
    fn name(&self) -> &str {
        "od"
    }
    fn sync_policy(&self) -> SyncPolicy {
        SyncPolicy::OnDemand
    }
    async fn ping(&self) -> SourceResult<()> {
        Ok(())
    }
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        match dir {
            "" => Ok(vec![
                SourceEntry::Dir("od/dir".to_owned()),
                SourceEntry::Song(make_test_song("od/root", 1)),
            ]),
            "dir" => Ok(vec![SourceEntry::Song(make_test_song("od/dir/one", 2))]),
            other => Err(SourceError::NotFound(other.to_owned())),
        }
    }
    async fn list_all(&self) -> SourceResult<Vec<rmpd_core::song::Song>> {
        Ok(Vec::new())
    }
    async fn search(&self, _q: &str) -> SourceResult<Vec<rmpd_core::song::Song>> {
        Ok(Vec::new())
    }
    async fn resolve_stream_uri(&self, id: &str) -> SourceResult<String> {
        Ok(format!("http://stream.example/{id}"))
    }
    async fn lookup(&self, uri: &str) -> SourceResult<Option<rmpd_core::song::Song>> {
        match uri {
            "od/root" | "od/dir/one" => Ok(Some(make_test_song("internal-id", 9))),
            _ => Ok(None),
        }
    }
}

fn state() -> AppState {
    let mut state = AppState::new();
    state.set_sources(Arc::new(SourceRegistry {
        sources: vec![Box::new(OnDemand) as Box<dyn MusicSource>],
    }));
    state
}

async fn queued_paths(state: &AppState) -> Vec<String> {
    let q = state.queue.read().await;
    (0..q.len() as u32)
        .filter_map(|p| q.get(p).map(|i| i.song.path.to_string()))
        .collect()
}

#[tokio::test]
async fn addid_queues_on_demand_song_with_requested_path() {
    let state = state();
    let resp = handle_addid_command(&state, "od/dir/one", None).await;
    assert!(resp.starts_with("Id: "), "unexpected response: {resp}");
    assert_eq!(queued_paths(&state).await, ["od/dir/one"]);

    // The queued path resolves through the source at playback time.
    let song = state.queue.read().await.get(0).unwrap().song.clone();
    let ps = rmpd_protocol::commands::utils::prepare_song_for_playback(
        &song,
        None,
        None,
        &state.sources,
    )
    .await
    .expect("resolve");
    assert_eq!(ps.resolved_path.as_str(), "http://stream.example/one");
}

#[tokio::test]
async fn addid_unknown_on_demand_song_is_no_such_song() {
    let state = state();
    let resp = handle_addid_command(&state, "od/nope", None).await;
    assert!(resp.starts_with("ACK"), "{resp}");
    assert!(resp.contains("No such song"), "{resp}");
    assert!(queued_paths(&state).await.is_empty());
}

#[tokio::test]
async fn add_song_and_directory_from_on_demand_source() {
    let state = state();
    let resp = handle_add_command(&state, "od/root", None).await;
    assert!(resp.starts_with("OK"), "{resp}");
    assert_eq!(queued_paths(&state).await, ["od/root"]);

    // Directory (and the mount root) expand recursively through browse.
    let resp = handle_add_command(&state, "od", None).await;
    assert!(resp.starts_with("OK"), "{resp}");
    assert_eq!(
        queued_paths(&state).await,
        ["od/root", "od/root", "od/dir/one"]
    );

    let resp = handle_add_command(&state, "od/missing", None).await;
    assert!(resp.starts_with("ACK"), "{resp}");
}

/// State with a scratch database + playlist dir holding `pl.m3u`.
fn playlist_state(dir: &tempfile::TempDir) -> AppState {
    let mut state = state();
    let db_path = dir.path().join("t.db").to_string_lossy().into_owned();
    state.db_pool = Some(rmpd_library::DbPool::new(&db_path).unwrap());
    state.db_path = Some(db_path);
    let pl_dir = dir.path().join("playlists");
    std::fs::create_dir_all(&pl_dir).unwrap();
    std::fs::write(pl_dir.join("pl.m3u"), "od/root\nod/dir/one\nod/gone\n").unwrap();
    state.playlist_dir = Some(pl_dir.to_string_lossy().into_owned());
    state
}

#[tokio::test]
async fn listplaylistinfo_resolves_on_demand_entries_with_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let state = playlist_state(&dir);
    let resp = handle_listplaylistinfo_command(&state, "pl", None).await;
    assert!(resp.trim_end().ends_with("OK"), "{resp}");
    // Resolved entries carry the requested path and the song's metadata
    // (`track` marks a lookup hit); the unknown one is a bare `file:` line.
    assert!(resp.contains("file: od/root\n"), "{resp}");
    assert!(resp.contains("file: od/dir/one\n"), "{resp}");
    assert!(resp.contains("file: od/gone\n"), "{resp}");
    assert!(!resp.contains("internal-id"), "{resp}");
    // Lookup hits emit more than the bare `file:` line; "od/gone" does not.
    assert!(resp.lines().count() > 4, "{resp}");
}

#[tokio::test]
async fn load_queues_on_demand_entries_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let state = playlist_state(&dir);
    let resp = handle_load_command(&state, "pl", None, None).await;
    assert!(resp.starts_with("OK"), "{resp}");
    assert_eq!(
        queued_paths(&state).await,
        ["od/root", "od/dir/one", "od/gone"]
    );
}
