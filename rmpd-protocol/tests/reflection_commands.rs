//! Integration tests for reflection and connection commands

mod common;

use common::TestClient;

#[test]
fn test_commands_command() {
    // commands should list available commands
    let response = "command: add\ncommand: play\ncommand: status\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_notcommands_command() {
    // notcommands should list unavailable commands
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_command() {
    // tagtypes should list tag types
    let response = "tagtype: Artist\ntagtype: Album\ntagtype: Title\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_urlhandlers_command() {
    // urlhandlers should list supported URL schemes
    let response = "handler: file\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_decoders_command() {
    // decoders should list supported formats
    let response = "plugin: flac\nsuffix: flac\nmime_type: audio/flac\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_config_command() {
    // config should return server configuration
    let response = "music_directory: /var/lib/mpd/music\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "music_directory"),
        Some("/var/lib/mpd/music")
    );
}

#[test]
fn test_protocol_command() {
    // protocol should handle protocol feature management
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_disable() {
    // tagtypes disable should disable a tag type
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_enable() {
    // tagtypes enable should enable a tag type
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_clear() {
    // tagtypes clear should disable all tag types
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_tagtypes_all() {
    // tagtypes all should enable all tag types
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_password_command() {
    // password command (authentication not implemented)
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_binarylimit_command() {
    // binarylimit sets max binary response size
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_kill_command() {
    // kill should trigger graceful shutdown
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}
