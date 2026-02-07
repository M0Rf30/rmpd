//! Tests for MPD command batching (command_list_begin / command_list_ok_begin).

use crate::tcp_harness::*;

#[tokio::test]
async fn command_list_basic() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list(&["ping", "ping", "ping"]).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn command_list_ok_basic() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list_ok(&["ping", "ping"]).await;
    // In ok mode, each successful command gets a "list_OK" separator
    let list_ok_count = resp.matches("list_OK").count();
    assert_eq!(
        list_ok_count, 2,
        "expected 2 list_OK separators, got: {resp}"
    );
    assert!(resp.ends_with("OK\n"), "batch must end with OK: {resp}");
}

#[tokio::test]
async fn command_list_error_stops_batch() {
    let (_server, mut client) = setup().await;
    // Second command is invalid — should stop batch with ACK
    let resp = client
        .command_list(&["ping", "not_a_real_command", "ping"])
        .await;
    assert!(resp.starts_with("ACK "), "batch error should return ACK");
    // The ACK should include the index of the failing command
    assert!(resp.contains("1@"), "ACK should reference index 1: {resp}");
}

#[tokio::test]
async fn command_list_ok_error_stops_batch() {
    let (_server, mut client) = setup().await;
    let resp = client
        .command_list_ok(&["ping", "not_a_real_command"])
        .await;
    assert!(
        resp.starts_with("ACK ") || resp.contains("ACK "),
        "batch error should return ACK: {resp}"
    );
}

#[tokio::test]
async fn command_list_end_without_begin() {
    let (_server, mut client) = setup().await;
    let resp = client.command("command_list_end").await;
    assert!(
        resp.starts_with("ACK "),
        "end without begin should be an error: {resp}"
    );
}

#[tokio::test]
async fn empty_command_list() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list(&[]).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn empty_command_list_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list_ok(&[]).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn command_list_with_status() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list_ok(&["status", "ping"]).await;
    // Should have list_OK after status output and after ping
    let list_ok_count = resp.matches("list_OK").count();
    assert_eq!(list_ok_count, 2, "expected 2 list_OK: {resp}");
    assert!(resp.ends_with("OK\n"));
}

#[tokio::test]
async fn command_list_preserves_order() {
    let (_server, mut client) = setup().await;
    let resp = client.command_list_ok(&["ping", "ping", "ping"]).await;
    // Should have 3 list_OK separators followed by final OK
    let list_ok_count = resp.matches("list_OK").count();
    assert_eq!(list_ok_count, 3, "expected 3 list_OK: {resp}");
    assert!(resp.ends_with("OK\n"));
}
