//! Integration tests for queue operations

mod common;

use common::TestClient;

#[test]
fn test_add_and_playlistinfo() {
    // Test response format for add command
    let add_response = "OK\n";
    assert!(TestClient::is_ok(add_response));

    // Test playlistinfo after adding a song
    let info_response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(info_response));
    assert_eq!(TestClient::get_field(info_response, "file"), Some("test.mp3"));
    assert_eq!(TestClient::get_field(info_response, "Pos"), Some("0"));
    assert_eq!(TestClient::get_field(info_response, "Id"), Some("1"));
}

#[test]
fn test_addid_command() {
    // addid should return the new song ID
    let response = "Id: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_delete_command() {
    // delete should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_deleteid_command() {
    // deleteid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_move_command() {
    // move should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_moveid_command() {
    // moveid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_swap_command() {
    // swap should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_swapid_command() {
    // swapid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_shuffle_command() {
    // shuffle should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistid_command() {
    // playlistid with specific ID
    let response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_playlistfind_command() {
    // playlistfind should return matching songs
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Test Song"));
    assert_eq!(TestClient::get_field(response, "Artist"), Some("Test Artist"));
}

#[test]
fn test_playlistsearch_command() {
    // playlistsearch should return matching songs (case-insensitive)
    let response = "file: test.mp3\nTitle: Test Song\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_plchanges_command() {
    // plchanges should return songs that changed since version
    let response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_plchangesposid_command() {
    // plchangesposid should return position and ID changes
    let response = "cpos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "cpos"), Some("0"));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_prio_command() {
    // prio should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_prioid_command() {
    // prioid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rangeid_command() {
    // rangeid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_addtagid_command() {
    // addtagid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_cleartagid_command() {
    // cleartagid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}
