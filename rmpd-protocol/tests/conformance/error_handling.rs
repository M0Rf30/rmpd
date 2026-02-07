//! Tests for MPD error handling: ACK format, malformed args, missing args.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn ack_format_has_code_and_command() {
    let (_server, mut client) = setup().await;
    let resp = client.command("not_a_real_command").await;
    // ACK format: "ACK [error@command_listNum] {current_command} message_text\n"
    assert!(resp.starts_with("ACK ["));
    assert!(resp.contains('{'));
    assert!(resp.contains('}'));
    assert!(resp.ends_with('\n'));
}

#[tokio::test]
async fn ack_for_missing_args() {
    let (_server, mut client) = setup().await;
    // "add" requires a URI argument
    let resp = client.command("add").await;
    assert!(resp.starts_with("ACK "), "missing arg should error: {resp}");
}

#[tokio::test]
async fn ack_for_invalid_volume() {
    let (_server, mut client) = setup().await;
    // Volume must be 0-100
    let resp = client.command("setvol 999").await;
    assert!(
        resp.starts_with("ACK "),
        "invalid volume should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_non_numeric_argument() {
    let (_server, mut client) = setup().await;
    let resp = client.command("setvol abc").await;
    assert!(
        resp.starts_with("ACK "),
        "non-numeric arg should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_delete_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("delete 0").await;
    assert!(
        resp.starts_with("ACK "),
        "delete on empty queue should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_play_out_of_range() {
    let (_server, mut client) = setup().await;
    let resp = client.command("play 999").await;
    assert!(
        resp.starts_with("ACK "),
        "play out of range should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_seekid_nonexistent() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seekid 9999 0").await;
    assert!(
        resp.starts_with("ACK "),
        "seekid non-existent should error: {resp}"
    );
}

#[tokio::test]
async fn ack_for_deleteid_nonexistent() {
    let (_server, mut client) = setup().await;
    let resp = client.command("deleteid 9999").await;
    assert!(
        resp.starts_with("ACK "),
        "deleteid non-existent should error: {resp}"
    );
}
