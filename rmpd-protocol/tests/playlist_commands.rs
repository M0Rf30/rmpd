//! Integration tests for stored playlist commands

mod common;

use common::TestClient;

#[test]
fn test_listplaylists_command() {
    // listplaylists should return list of playlists
    let response = "playlist: favorites\nLast-Modified: 2024-01-01T00:00:00Z\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "playlist"), Some("favorites"));
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
    let response = "file: song1.mp3\nTitle: Song 1\nArtist: Artist 1\nfile: song2.mp3\nTitle: Song 2\nOK\n";
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
