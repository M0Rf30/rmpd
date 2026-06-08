//! Regression tests for the idle / noidle stream-synchronization race.
//!
//! Background
//! ----------
//! MPD clients such as rmpc drive idle with two cooperating tasks sharing one
//! socket: an *idle* task that sends `idle` and reads the idle response, and a
//! *request* task that, when it has work to do, writes `noidle` directly to the
//! socket and then issues real commands.
//!
//! When an idle event is delivered (the server flushes `changed: <sub>\nOK\n`)
//! at the same moment the client writes `noidle`, the server receives `noidle`
//! while it is *no longer* idling. Per MPD's `Client::ProcessLine`, the server
//! must write **nothing** in that case — the client has already received the
//! full idle response. Emitting a spurious `OK` here injects an extra line into
//! the stream, desynchronizing every subsequent response. rmpc surfaces this as
//! `Expected 'OK' but got '<value>'`.
//!
//! These tests pin that behavior.

use rmpd_core::event::Event;
use rmpd_core::state::PlayerState;
use rmpd_protocol::state::AppState;

#[path = "common/tcp_harness.rs"]
mod tcp_harness;
use tcp_harness::{MpdTestClient, MpdTestServer};

/// A `noidle` received while the connection is NOT idling (because an idle
/// event was already delivered) must produce no output. The follow-up command
/// must therefore read its own response cleanly, with no leading stray `OK`.
#[tokio::test]
async fn noidle_after_event_does_not_emit_stray_ok() {
    let state = AppState::new();
    // Clone the event bus so we can emit events into the running server.
    let event_bus = state.event_bus.clone();

    let server = MpdTestServer::start_with_state(state).await;
    let mut client = MpdTestClient::connect(server.port()).await;

    // 1. Enter idle.
    client.send_raw("idle\n").await;
    // Give the server a moment to enter the idle select loop.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 2. An event fires: the server flushes the idle response and leaves idle.
    event_bus.emit(Event::PlayerStateChanged(PlayerState::Play));

    // 3. The client reads the idle response.
    let idle_resp = client.read_response().await;
    assert_eq!(
        idle_resp, "changed: player\nOK\n",
        "idle should report the player subsystem then OK"
    );

    // 4. The client (racing) sends noidle — the server is no longer idling.
    //    Real MPD writes nothing here.
    client.send_raw("noidle\n").await;

    // 5. The next command must read its OWN response. If the server emitted a
    //    spurious OK for the stale noidle, `read_response` would return that
    //    bare "OK\n" instead of the status body.
    let status_resp = client.command("status").await;

    assert!(
        status_resp.contains("state:"),
        "status response was desynchronized by a stray noidle OK: {status_resp:?}"
    );
    assert!(
        status_resp.ends_with("OK\n"),
        "status response should terminate with OK: {status_resp:?}"
    );
}

/// A bare `noidle` sent when the client never entered idle mode must likewise
/// produce no output (matching MPD), so the following command stays in sync.
#[tokio::test]
async fn bare_noidle_without_idle_emits_nothing() {
    let (_server, mut client) = {
        let server = MpdTestServer::start().await;
        let client = MpdTestClient::connect(server.port()).await;
        (server, client)
    };

    // Never idled; send noidle anyway. No response expected.
    client.send_raw("noidle\n").await;

    // The ping response must be exactly OK — not preceded by a stray noidle OK.
    let resp = client.command("ping").await;
    assert_eq!(resp, "OK\n", "bare noidle must not emit its own OK: {resp:?}");
}
