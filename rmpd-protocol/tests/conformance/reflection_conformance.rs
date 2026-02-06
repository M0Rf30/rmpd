//! Tests for MPD reflection commands over TCP.

use crate::common::tcp_harness::*;

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
