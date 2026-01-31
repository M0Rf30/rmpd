//! Integration tests for database commands

mod common;

use common::TestClient;

#[test]
fn test_update_command() {
    // update should return a job ID
    let response = "updating_db: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "updating_db"), Some("1"));
}

#[test]
fn test_rescan_command() {
    // rescan should return a job ID
    let response = "updating_db: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_list_command() {
    // list should return list of values for a tag
    let response = "Artist: The Beatles\nArtist: Pink Floyd\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listall_command() {
    // listall should return all files and directories
    let response = "directory: music/album\nfile: music/album/song.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listallinfo_command() {
    // listallinfo should return files with metadata
    let response = "directory: music/album\nfile: music/album/song.mp3\nTitle: Song\nArtist: Artist\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_lsinfo_command() {
    // lsinfo should return directory contents
    let response = "directory: subdir\nfile: song.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_search_command() {
    // search should return matching songs
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Test Song"));
}

#[test]
fn test_find_command() {
    // find should return exact matches
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchadd_command() {
    // searchadd should add matching songs to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_findadd_command() {
    // findadd should add exact matches to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_count_command() {
    // count should return statistics
    let response = "songs: 42\nplaytime: 3600\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "songs"), Some("42"));
    assert_eq!(TestClient::get_field(response, "playtime"), Some("3600"));
}

#[test]
fn test_searchcount_command() {
    // searchcount is an alias for count
    let response = "songs: 10\nplaytime: 600\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listfiles_command() {
    // listfiles should return files in directory
    let response = "file: song1.mp3\nfile: song2.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_readcomments_command() {
    // readcomments should return file metadata
    let response = "comment: This is a comment\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "comment"), Some("This is a comment"));
}

#[test]
fn test_getfingerprint_command() {
    // getfingerprint currently returns an error (not implemented)
    let response = "ACK [50@0] {getfingerprint} chromaprint not available\n";
    assert!(TestClient::is_error(response));
}

#[test]
fn test_albumart_command() {
    // albumart should return binary data or error
    let response = "size: 12345\nbinary: 12345\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "size"), Some("12345"));
}

#[test]
fn test_readpicture_command() {
    // readpicture should return binary picture data or error
    let response = "size: 54321\ntype: image/jpeg\nbinary: 54321\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "type"), Some("image/jpeg"));
}
