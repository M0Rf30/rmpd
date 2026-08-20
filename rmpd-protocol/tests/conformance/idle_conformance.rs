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

#[tokio::test]
async fn idle_triggered_by_addid() {
    // Regression: `addid` must wake an idling client on the `playlist`
    // subsystem, otherwise event-driven clients (rmpc) never refetch the queue
    // and it appears empty even though playback started.
    let (server, mut adder, _tmp) = setup_with_db(3).await;
    let mut idler = MpdTestClient::connect(server.port()).await;

    idler.send_raw("idle\n").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = adder.command("addid \"music/song1.flac\"").await;
    assert!(
        get_field(&resp, "Id").is_some(),
        "addid must return Id: {resp}"
    );

    let idle_resp = idler.read_response().await;
    assert!(
        idle_resp.contains("changed: playlist"),
        "addid should notify the playlist idle subsystem, got: {idle_resp}"
    );
    assert_ok(&idle_resp);
}

#[tokio::test]
async fn idle_triggered_by_add() {
    let (server, mut adder, _tmp) = setup_with_db(3).await;
    let mut idler = MpdTestClient::connect(server.port()).await;

    idler.send_raw("idle playlist\n").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp = adder.command("add \"music/song1.flac\"").await;
    assert_ok(&resp);

    let idle_resp = idler.read_response().await;
    assert!(
        idle_resp.contains("changed: playlist"),
        "add should notify the playlist idle subsystem, got: {idle_resp}"
    );
    assert_ok(&idle_resp);
}

#[tokio::test]
async fn single_connection_idle_sees_buffered_addid() {
    // Faithful to rmpc's usage: one connection that cycles idle -> noidle ->
    // command -> idle. A queue change made between idle calls must be buffered
    // and reported on the next idle.
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client.send_raw("idle\n").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.send_raw("noidle\n").await;
    let r = client.read_response().await;
    assert_ok(&r);

    let resp = client.command("addid \"music/song1.flac\"").await;
    assert!(
        get_field(&resp, "Id").is_some(),
        "addid must return Id: {resp}"
    );

    // Re-entering idle must immediately report the buffered playlist change.
    let idle_resp = client.command("idle").await;
    assert!(
        idle_resp.contains("changed: playlist"),
        "next idle should report the buffered playlist change, got: {idle_resp}"
    );
    assert_ok(&idle_resp);
}

#[tokio::test]
async fn idle_coalesces_simultaneous_subsystem_changes() {
    // Regression: protocol.rst command_idle says idle "lists all changed
    // systems in a line" — MPD accumulates idle flags and reports every
    // pending subsystem in one reply. rmpd used to return only the first
    // matching event and silently drop the rest until the next idle.
    //
    // Both changes are made while the client is NOT idling, so they are
    // already buffered by the time `idle` is sent — no race between the
    // triggering commands and the idler's wakeup.
    let (server, mut trigger, _tmp) = setup_with_db(1).await;
    let mut idler = MpdTestClient::connect(server.port()).await;

    let r1 = trigger.command("toggleoutput 0").await;
    assert_ok(&r1);
    let r2 = trigger.command("addid \"music/song1.flac\"").await;
    assert!(get_field(&r2, "Id").is_some(), "addid must return Id: {r2}");

    let idle_resp = idler.command("idle").await;
    assert!(
        idle_resp.contains("changed: output") && idle_resp.contains("changed: playlist"),
        "idle must report every buffered subsystem in one reply, got: {idle_resp}"
    );
    assert_ok(&idle_resp);
}
