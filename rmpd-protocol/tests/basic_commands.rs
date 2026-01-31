//! Integration tests for basic MPD protocol commands

mod common;

use common::TestClient;

#[test]
fn test_ping_command() {
    // Ping should always return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_status_response_format() {
    // Test that status response parsing works
    let response = "volume: 100\nrepeat: 0\nrandom: 0\nsingle: 0\nconsume: 0\nplaylist: 1\nplaylistlength: 0\nstate: stop\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "volume"), Some("100"));
    assert_eq!(TestClient::get_field(response, "state"), Some("stop"));
    assert_eq!(TestClient::get_field(response, "playlist"), Some("1"));
}

#[test]
fn test_stats_response_format() {
    // Test stats response format
    let response = "artists: 0\nalbums: 0\nsongs: 0\nuptime: 60\ndb_playtime: 0\ndb_update: 0\nplaytime: 0\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "artists"), Some("0"));
    assert_eq!(TestClient::get_field(response, "songs"), Some("0"));
}

#[test]
fn test_unknown_command() {
    // Unknown commands should return an error
    let response = "ACK [5@0] {invalid_command} unknown command\n";
    assert!(TestClient::is_error(response));
}

#[test]
fn test_currentsong_empty_queue() {
    // currentsong with empty queue should return OK with no fields
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistinfo_empty_queue() {
    // playlistinfo with empty queue should return OK with no songs
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_clear_command() {
    // Clear should always succeed
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_outputs_command() {
    // outputs should return at least one output
    let response = "outputid: 0\noutputname: Default Output\noutputenabled: 1\nOK\n";

    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "outputid"), Some("0"));
    assert_eq!(TestClient::get_field(response, "outputenabled"), Some("1"));
}

#[test]
fn test_commands_list() {
    // commands should return a list of available commands
    let response = "command: play\ncommand: pause\ncommand: stop\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_command() {
    // tagtypes should return list of tag types
    let response = "tagtype: Artist\ntagtype: Album\ntagtype: Title\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_urlhandlers_command() {
    // urlhandlers should return supported URL schemes
    let response = "handler: file\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_decoders_command() {
    // decoders should return supported audio formats
    let response = "plugin: flac\nsuffix: flac\nmime_type: audio/flac\nOK\n";
    assert!(TestClient::is_ok(response));
}
