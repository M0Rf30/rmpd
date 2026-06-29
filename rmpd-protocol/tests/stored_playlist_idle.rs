use rmpd_core::event::{Event, Subsystem};
use rmpd_protocol::commands::playlists;
use rmpd_protocol::state::AppState;

#[test]
fn stored_playlist_event_maps_to_subsystem() {
    assert!(
        Event::StoredPlaylistChanged
            .subsystems()
            .contains(&Subsystem::StoredPlaylist),
        "Event::StoredPlaylistChanged must map to Subsystem::StoredPlaylist"
    );
}

#[tokio::test]
async fn save_emits_stored_playlist_notification() {
    let tmp = tempfile::TempDir::new().unwrap();
    let music = tmp.path().join("music");
    let playlists_dir = tmp.path().join("playlists");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&playlists_dir).unwrap();

    let state = AppState::with_all_paths(
        tmp.path().join("db").to_str().unwrap().to_string(),
        music.to_str().unwrap().to_string(),
        playlists_dir.to_str().unwrap().to_string(),
    );

    let mut rx = state.event_bus.subscribe();

    let resp = playlists::handle_save_command(&state, "mylist", None).await;
    assert!(resp.contains("OK"), "save should succeed, got: {resp}");

    let mut got = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, Event::StoredPlaylistChanged) {
            got = true;
        }
    }

    assert!(
        got,
        "save must emit Event::StoredPlaylistChanged so idle stored_playlist clients are notified"
    );
}
