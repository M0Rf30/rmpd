//! Tests for MPD reflection commands over TCP.

use crate::tcp_harness::*;
use rmpd_protocol::state::AppState;

#[tokio::test]
async fn commands_lists_available() {
    let (_server, mut client) = setup().await;
    let resp = client.command("commands").await;
    assert_ok(&resp);
    // Should contain well-known commands
    assert!(resp.contains("command: play"), "should list play: {resp}");
    assert!(resp.contains("command: status"), "should list status");
    assert!(resp.contains("command: ping"), "should list ping");
}

#[tokio::test]
async fn commands_and_notcommands_partition_by_permission() {
    // With a password configured, an unauthenticated connection starts with
    // zero permissions, so PLAYER-gated commands (e.g. "play") must appear
    // in `notcommands` and not in `commands`, while NONE-gated commands
    // (e.g. "ping") always appear in `commands`.
    let mut state = AppState::new();
    state.password = Some("secret".to_string());
    let (_server, mut client) = setup_with_state(state).await;

    let commands = client.command("commands").await;
    assert_ok(&commands);
    assert!(
        commands.contains("command: ping"),
        "ping is PERMISSION_NONE: {commands}"
    );
    assert!(
        !commands.contains("command: play"),
        "play requires permission: {commands}"
    );

    let notcommands = client.command("notcommands").await;
    assert_ok(&notcommands);
    assert!(
        notcommands.contains("command: play"),
        "play should be listed as unavailable: {notcommands}"
    );
    assert!(
        !notcommands.contains("command: ping"),
        "ping should not be listed as unavailable: {notcommands}"
    );

    // After authenticating, play becomes available and is no longer in notcommands.
    let auth = client.command("password secret").await;
    assert_ok(&auth);
    let commands = client.command("commands").await;
    assert!(
        commands.contains("command: play"),
        "play should be available after auth: {commands}"
    );
    let notcommands = client.command("notcommands").await;
    assert!(
        !notcommands.contains("command: play"),
        "play should no longer be unavailable after auth: {notcommands}"
    );
}

#[tokio::test]
async fn permission_denied_ack_matches_mpd() {
    // A PLAYER-gated command without permission must return MPD's exact
    // ACK code (4 = ACK_ERROR_PERMISSION) and message text.
    let mut state = AppState::new();
    state.password = Some("secret".to_string());
    let (_server, mut client) = setup_with_state(state).await;

    let resp = client.command("play").await;
    assert!(
        resp.contains("ACK [4@0] {play} you don't have permission for \"play\""),
        "unexpected ACK: {resp}"
    );
}

#[tokio::test]
async fn idle_requires_read_permission() {
    // idle bypasses handle_command's generic permission check (it needs
    // raw reader/event_rx access for long-poll), so it must be gated
    // separately; verify it still enforces PERMISSION_READ.
    let mut state = AppState::new();
    state.password = Some("secret".to_string());
    let (_server, mut client) = setup_with_state(state).await;

    let resp = client.command("idle").await;
    assert!(
        resp.contains("ACK [4@0] {idle} you don't have permission for \"idle\""),
        "unexpected ACK: {resp}"
    );

    let auth = client.command("password secret").await;
    assert_ok(&auth);

    client.send_raw("idle\n").await;
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    client.send_raw("noidle\n").await;
    let resp = client.read_response().await;
    assert_ok(&resp);
}

#[tokio::test]
async fn notcommands_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("notcommands").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn tagtypes_lists_tags() {
    let (_server, mut client) = setup().await;
    let resp = client.command("tagtypes").await;
    assert_ok(&resp);
    assert!(resp.contains("tagtype:"), "should list tag types: {resp}");
    assert!(resp.contains("Artist"), "should include Artist");
}

#[tokio::test]
async fn tagtypes_disable_and_enable() {
    let (_server, mut client) = setup().await;

    let resp = client.command("tagtypes disable Artist").await;
    assert_ok(&resp);

    let resp = client.command("tagtypes enable Artist").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn tagtypes_clear_and_all() {
    let (_server, mut client) = setup().await;

    let resp = client.command("tagtypes clear").await;
    assert_ok(&resp);

    // After clear, tagtypes should return no tags
    let resp = client.command("tagtypes").await;
    assert_ok(&resp);

    let resp = client.command("tagtypes all").await;
    assert_ok(&resp);

    // After all, tagtypes should return tags again
    let resp = client.command("tagtypes").await;
    assert!(resp.contains("tagtype:"), "should have tags after 'all'");
}

#[tokio::test]
async fn urlhandlers_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("urlhandlers").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn decoders_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("decoders").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn config_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("config").await;
    // config may return OK or ACK depending on local vs network
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn protocol_clear_and_all() {
    let (_server, mut client) = setup().await;

    let resp = client.command("protocol clear").await;
    assert_ok(&resp);

    let resp = client.command("protocol all").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn protocol_disable_and_enable() {
    let (_server, mut client) = setup().await;

    let resp = client.command("protocol disable binary").await;
    assert_ok(&resp);

    let resp = client.command("protocol enable binary").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn binarylimit_rejects_too_small() {
    let (_server, mut client) = setup().await;

    let resp = client.command("binarylimit 32").await;
    assert!(resp.starts_with("ACK [2@0]"), "got: {resp}");

    let resp = client.command("binarylimit 8192").await;
    assert_ok(&resp);
}
