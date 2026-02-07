//! Tests for MPD idle/noidle commands over TCP.

use crate::tcp_harness::*;
use tokio::time::Duration;

#[tokio::test]
async fn noidle_returns_ok() {
    let (_server, mut client) = setup().await;
    // Send idle then immediately noidle
    client.send_raw("idle\n").await;
    // Small delay to let idle register
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.send_raw("noidle\n").await;

    let resp = client.read_response().await;
    assert_ok(&resp);
}

#[tokio::test]
async fn idle_with_subsystem_filter() {
    let (_server, mut client) = setup().await;
    client.send_raw("idle player\n").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.send_raw("noidle\n").await;

    let resp = client.read_response().await;
    assert_ok(&resp);
}

#[tokio::test]
async fn idle_triggered_by_output_change() {
    let server = MpdTestServer::start().await;
    let mut client1 = MpdTestClient::connect(server.port()).await;
    let mut client2 = MpdTestClient::connect(server.port()).await;

    // Client 1 enters idle waiting for output changes
    client1.send_raw("idle output\n").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client 2 toggles output (this emits an OutputsChanged event)
    let resp = client2.command("toggleoutput 0").await;
    assert_ok(&resp);

    // Client 1 should receive the change notification
    let resp = client1.read_response().await;
    assert!(
        resp.contains("changed:"),
        "idle should report change: {resp}"
    );
    assert_ok(&resp);
}

#[tokio::test]
async fn idle_noidle_is_idempotent() {
    let (_server, mut client) = setup().await;

    // Send idle then noidle, twice in succession
    for _ in 0..3 {
        client.send_raw("idle\n").await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        client.send_raw("noidle\n").await;
        let resp = client.read_response().await;
        assert_ok(&resp);
    }
}

#[tokio::test]
async fn idle_multiple_subsystems() {
    let (_server, mut client) = setup().await;
    client.send_raw("idle player mixer options\n").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.send_raw("noidle\n").await;

    let resp = client.read_response().await;
    assert_ok(&resp);
}
