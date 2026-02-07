//! Extended playback command conformance tests.
//! Tests pause toggle modes and seekcur relative seeking.

use crate::tcp_harness::*;

#[tokio::test]
async fn pause_toggle_returns_ok() {
    let (_server, mut client) = setup().await;
    // pause (no arg) when stopped is a toggle — no-op when stopped, returns OK
    let resp = client.command("pause").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn pause_explicit_1_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("pause 1").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn pause_explicit_0_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("pause 0").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn seekcur_relative_positive_not_playing() {
    let (_server, mut client) = setup().await;
    // seekcur with relative offset when not playing should error (no current song)
    let resp = client.command("seekcur +10").await;
    assert!(
        resp.starts_with("ACK "),
        "seekcur +10 when stopped should ACK: {resp}"
    );
}

#[tokio::test]
async fn seekcur_relative_negative_not_playing() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seekcur -5").await;
    assert!(
        resp.starts_with("ACK "),
        "seekcur -5 when stopped should ACK: {resp}"
    );
}
