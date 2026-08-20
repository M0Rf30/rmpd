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
    assert!(resp.contains("@1"), "ACK should reference index 1: {resp}");
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
    // `command_list_end` isn't in MPD's `commands[]` table; outside a list
    // it's looked up like any other name and reported as unknown, exactly as
    // MPD does (`ACK_ERROR_NOT_LIST` is declared but never actually thrown).
    assert_eq!(resp, "ACK [5@0] {} unknown command \"command_list_end\"\n");
}

#[tokio::test]
async fn idle_inside_command_list_closes_connection() {
    let (_server, mut client) = setup().await;
    // MPD: idle/noidle are async commands and can't be used inside a command
    // list; the connection is closed immediately with no ACK at all
    // (Client::ProcessLine's IsAsyncCommmand check runs before the list is
    // ever executed).
    client
        .send_raw("command_list_begin\nidle\ncommand_list_end\n")
        .await;
    let line = client.read_line().await;
    assert!(
        line.is_empty(),
        "idle inside a command list should close the connection: {line:?}"
    );
}

#[tokio::test]
async fn close_inside_command_list_closes_connection_without_panicking() {
    let (_server, mut client) = setup().await;
    // MPD's `close` short-circuits list execution (`CommandResult::FINISH`)
    // and closes the connection; it must not reach the normal per-command
    // dispatch (which has no handler for `Close`, since it's always meant
    // to be intercepted first).
    let resp = client.command_list(&["status", "close", "ping"]).await;
    // The first command's output is flushed before `close`, but the list
    // never completes, so there's no trailing "OK" for it.
    assert!(
        !resp.ends_with("OK\n"),
        "list should not complete: {resp:?}"
    );
    let line = client.read_line().await;
    assert!(
        line.is_empty(),
        "close inside a command list should close the connection: {line:?}"
    );
}

#[tokio::test]
async fn nested_command_list_begin_fails_as_unknown_command() {
    let (_server, mut client) = setup().await;
    // MPD has no nesting check: `command_list_begin` is only special-cased
    // at the top level while *not* already in a list. Once inside one, it
    // is just another line appended to the list, which then fails
    // `command_lookup` (it isn't in `commands[]` either) when the list
    // executes.
    client
        .send_raw("command_list_begin\ncommand_list_begin\ncommand_list_end\n")
        .await;
    let resp = client.read_response().await;
    assert_eq!(
        resp,
        "ACK [5@0] {} unknown command \"command_list_begin\"\n"
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
