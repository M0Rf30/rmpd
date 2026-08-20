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

// ── Real behavioral tests for fixed protocol-compliance gaps ──────────
// (the tests above are pre-existing placeholders that assert on literal
// strings, not handler output; these call the actual handlers.)

use rmpd_protocol::commands::connection;
use rmpd_protocol::commands::reflection;
use rmpd_protocol::parser::{StringNormalizationSubcommand, TagTypesSubcommand};
use rmpd_protocol::{AppState, ConnectionState};

#[tokio::test]
async fn tagtypes_available_lists_all_tags_even_when_disabled() {
    let mut conn_state = ConnectionState::new();
    conn_state.disable_all_tags();

    let bare = reflection::handle_tagtypes_command(&mut conn_state, None).await;
    assert!(
        !bare.contains("tagtype: Artist"),
        "bare tagtypes must respect the disabled set: {bare}"
    );

    let available =
        reflection::handle_tagtypes_command(&mut conn_state, Some(TagTypesSubcommand::Available))
            .await;
    assert!(
        available.contains("tagtype: Artist") && available.contains("tagtype: DiscSubtitle"),
        "available must list every globally-advertised tag regardless of enabled state: {available}"
    );
}

#[tokio::test]
async fn tagtypes_never_advertises_comment() {
    // MPD's global_tag_mask excludes Comment by default (tag/Settings.cxx);
    // it never appears in `tagtypes` or `tagtypes available`, even after
    // `tagtypes enable Comment` or `tagtypes all`.
    let mut conn_state = ConnectionState::new();

    let available =
        reflection::handle_tagtypes_command(&mut conn_state, Some(TagTypesSubcommand::Available))
            .await;
    assert!(
        !available.contains("tagtype: Comment"),
        "available must not advertise Comment: {available}"
    );

    reflection::handle_tagtypes_command(
        &mut conn_state,
        Some(TagTypesSubcommand::Enable {
            tags: vec!["Comment".to_string()],
        }),
    )
    .await;
    reflection::handle_tagtypes_command(&mut conn_state, Some(TagTypesSubcommand::All)).await;

    let bare = reflection::handle_tagtypes_command(&mut conn_state, None).await;
    assert!(
        !bare.contains("tagtype: Comment"),
        "bare tagtypes must not surface Comment even after enable/all: {bare}"
    );
}

#[tokio::test]
async fn stringnormalization_enable_disable_and_available() {
    let mut conn_state = ConnectionState::new();

    let bare = reflection::handle_stringnormalization_command(&mut conn_state, None).await;
    assert_eq!(bare, "OK\n", "no normalization enabled by default: {bare}");

    let available = reflection::handle_stringnormalization_command(
        &mut conn_state,
        Some(StringNormalizationSubcommand::Available),
    )
    .await;
    assert!(available.contains("stringnormalization: strip_diacritics"));

    let enabled = reflection::handle_stringnormalization_command(
        &mut conn_state,
        Some(StringNormalizationSubcommand::Enable {
            options: vec!["strip_diacritics".to_string()],
        }),
    )
    .await;
    assert_eq!(enabled, "OK\n");

    let bare = reflection::handle_stringnormalization_command(&mut conn_state, None).await;
    assert!(bare.contains("stringnormalization: strip_diacritics"));

    let unknown = reflection::handle_stringnormalization_command(
        &mut conn_state,
        Some(StringNormalizationSubcommand::Enable {
            options: vec!["bogus".to_string()],
        }),
    )
    .await;
    assert!(unknown.contains("ACK"));
}

#[tokio::test]
async fn config_rejects_remote_clients() {
    let state = AppState::new();
    let mut conn_state = ConnectionState::new();
    conn_state.is_local = false;

    let resp = connection::handle_config_command(&state, &conn_state).await;
    assert!(resp.contains("ACK"));
    assert!(resp.contains("Command only permitted to local clients"));
}

#[tokio::test]
async fn config_allows_local_clients() {
    let state = AppState::with_paths(
        "/tmp/rmpd_test_db".to_string(),
        "/tmp/rmpd_test_music".to_string(),
    );
    let mut conn_state = ConnectionState::new();
    conn_state.is_local = true;

    let resp = connection::handle_config_command(&state, &conn_state).await;
    assert!(resp.ends_with("OK\n"), "got: {resp}");
    assert!(resp.contains("music_directory: /tmp/rmpd_test_music"));
}

#[tokio::test]
async fn urlhandlers_hides_file_scheme_from_remote_clients() {
    let mut conn_state = ConnectionState::new();
    conn_state.is_local = false;
    let remote = reflection::handle_urlhandlers_command(&conn_state).await;
    assert!(!remote.contains("file://"), "got: {remote}");
    assert!(remote.contains("handler: http://"));
    assert!(remote.contains("handler: https://"));

    conn_state.is_local = true;
    let local = reflection::handle_urlhandlers_command(&conn_state).await;
    assert!(local.contains("handler: file://"), "got: {local}");
}
