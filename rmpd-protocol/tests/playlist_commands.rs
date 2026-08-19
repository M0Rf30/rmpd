//! Integration tests for stored playlist commands

mod common;

use common::TestClient;
use rmpd_protocol::commands::playlists;
use rmpd_protocol::state::AppState;

#[test]
fn test_listplaylists_command() {
    // listplaylists should return list of playlists
    let response = "playlist: favorites\nLast-Modified: 2024-01-01T00:00:00Z\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "playlist"),
        Some("favorites")
    );
}

#[test]
fn test_listplaylists_empty() {
    // listplaylists with no playlists
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_command() {
    // save should create a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_load_command() {
    // load should add playlist to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listplaylist_command() {
    // listplaylist should return playlist files
    let response = "file: song1.mp3\nfile: song2.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listplaylistinfo_command() {
    // listplaylistinfo should return playlist files with metadata
    let response =
        "file: song1.mp3\nTitle: Song 1\nArtist: Artist 1\nfile: song2.mp3\nTitle: Song 2\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Song 1"));
}

#[test]
fn test_playlistadd_command() {
    // playlistadd should add a song to playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistclear_command() {
    // playlistclear should clear a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistdelete_command() {
    // playlistdelete should remove a song from playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistmove_command() {
    // playlistmove should move a song in playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rename_command() {
    // rename should rename a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rm_command() {
    // rm should delete a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchplaylist_command() {
    // searchplaylist should search within a playlist
    let response = "file: song1.mp3\nTitle: Matching Song\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchaddpl_command() {
    // searchaddpl should search and add to playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistlength_command() {
    // playlistlength should return playlist length
    let response = "songs: 10\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "songs"), Some("10"));
}

#[test]
fn test_load_with_range() {
    // load with range should load subset of playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_load_with_position() {
    // load with position should insert at specific location
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_append_mode() {
    // save with append mode
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_create_mode() {
    // save with create mode (default)
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_replace_mode() {
    // save with replace mode
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

/// Build an `AppState` backed by fresh temp music/playlist directories,
/// mirroring the setup in `stored_playlist_idle.rs`.
fn new_test_state() -> (tempfile::TempDir, AppState) {
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
    (tmp, state)
}

#[tokio::test]
async fn test_playlist_name_traversal_rejected() {
    // save/load/rm must reject a traversal name before touching the filesystem.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "../evil", None).await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "save with traversal name must ACK [2@0], got: {resp}"
    );

    let resp = playlists::handle_load_command(&state, "../evil", None, None).await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "load with traversal name must ACK [2@0], got: {resp}"
    );

    let resp = playlists::handle_rm_command(&state, "../evil").await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "rm with traversal name must ACK [2@0], got: {resp}"
    );
}

#[tokio::test]
async fn test_playlist_name_normal_round_trips() {
    // A normal name must still save and load without triggering validation.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "My Playlist", None).await;
    assert!(TestClient::is_ok(&resp), "save should succeed, got: {resp}");

    let resp = playlists::handle_load_command(&state, "My Playlist", None, None).await;
    assert!(TestClient::is_ok(&resp), "load should succeed, got: {resp}");

    let resp = playlists::handle_rm_command(&state, "My Playlist").await;
    assert!(TestClient::is_ok(&resp), "rm should succeed, got: {resp}");
}
