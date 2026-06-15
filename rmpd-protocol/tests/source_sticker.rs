//! Source-backed songs (e.g. Subsonic) must support the full sticker flow and
//! expose a consistent virtual path across `lsinfo`/`listallinfo`, so clients
//! (mympd) can attach ratings/likes to them.

#[path = "common/tcp_harness.rs"]
mod tcp_harness;
use rmpd_core::song::{Song, intern_tag_key};
use rmpd_protocol::state::AppState;
use std::time::Duration;
use tcp_harness::*;
use tempfile::TempDir;

#[tokio::test]
async fn source_song_supports_stickers_and_consistent_path() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();
    let music_dir = tmp.path().join("music");
    std::fs::create_dir_all(&music_dir).unwrap();
    let playlist_dir = tmp.path().join("playlists");
    std::fs::create_dir_all(&playlist_dir).unwrap();

    // A realistic synced Subsonic track: mount-style path with spaces, unicode
    // and a file extension.
    let stored_path = "alarm-music/Ólafur Arnalds/some kind of peace/wfe3irg7kPs6aIpVblSP6k.flac";
    {
        let db = rmpd_library::Database::open(&db_path_str).unwrap();
        let song = Song {
            id: 0,
            path: stored_path.into(),
            duration: Some(Duration::from_secs(200)),
            sample_rate: Some(44100),
            channels: Some(2),
            bits_per_sample: Some(16),
            bitrate: Some(1000),
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
            tags: vec![
                (intern_tag_key("title"), "Loftið".into()),
                (intern_tag_key("artist"), "Ólafur Arnalds".into()),
                (intern_tag_key("album"), "some kind of peace".into()),
            ],
        };
        db.add_source_song(&song, "subsonic:alarm-music").unwrap();
    }

    let mut state = AppState::with_all_paths(
        db_path_str,
        music_dir.to_str().unwrap().to_string(),
        playlist_dir.to_str().unwrap().to_string(),
    );
    state.disable_actual_mount = true;
    let server = MpdTestServer::start_with_state(state).await;
    let mut client = MpdTestClient::connect(server.port()).await;

    // lsinfo and listallinfo must agree on the exact (mount-style) path.
    let ls = client
        .command("lsinfo \"alarm-music/Ólafur Arnalds/some kind of peace\"")
        .await;
    let emitted = get_field(&ls, "file")
        .expect("lsinfo emits file")
        .to_string();
    assert_eq!(emitted, stored_path);

    let lai = client.command("listallinfo").await;
    assert_eq!(
        get_field(&lai, "file"),
        Some(stored_path),
        "listallinfo must emit the same path as lsinfo/storage"
    );

    // Full sticker round-trip on the source song, with the path quoted as a
    // libmpdclient client (mympd) would send it.
    assert_ok(
        &client
            .command(&format!("sticker set song \"{emitted}\" rating \"8\""))
            .await,
    );
    let list = client
        .command(&format!("sticker list song \"{emitted}\""))
        .await;
    assert_ok(&list);
    assert!(list.contains("sticker: rating=8"), "got: {list}");
}
