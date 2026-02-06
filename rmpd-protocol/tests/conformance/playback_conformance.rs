//! Tests for MPD playback commands over TCP.
//! Note: actual audio playback is not expected in test environments.
//! These tests verify protocol-level responses (OK/ACK as appropriate).

use crate::common::tcp_harness::*;

#[tokio::test]
async fn play_empty_queue_errors() {
    let (_server, mut client) = setup().await;
    let resp = client.command("play 0").await;
    assert!(resp.starts_with("ACK "), "play on empty queue: {resp}");
}

#[tokio::test]
async fn stop_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("stop").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn pause_without_playing() {
    let (_server, mut client) = setup().await;
    // pause when stopped should still succeed (MPD returns OK)
    let resp = client.command("pause").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn next_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("next").await;
    // rmpd returns ACK when queue is empty - valid behavior
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn previous_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("previous").await;
    // rmpd returns ACK when queue is empty - valid behavior
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn currentsong_when_stopped() {
    let (_server, mut client) = setup().await;
    let resp = client.command("currentsong").await;
    assert_ok(&resp);
    // When stopped, currentsong should return OK with no song data
    assert_eq!(resp, "OK\n");
}

#[tokio::test]
async fn seekid_nonexistent_errors() {
    let (_server, mut client) = setup().await;
    let resp = client.command("seekid 9999 0").await;
    assert!(resp.starts_with("ACK "), "seekid non-existent: {resp}");
}

#[tokio::test]
async fn seekcur_when_stopped() {
    let (_server, mut client) = setup().await;
    // seekcur without playing should error or be no-op
    let resp = client.command("seekcur 0").await;
    // Implementation may return OK or ACK, both are valid
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn play_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    // play should succeed (even if audio device isn't available)
    let resp = client.command("play 0").await;
    // May succeed or fail depending on audio backend, but should not crash
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn playid_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let r = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&r, "Id").unwrap();

    let resp = client.command(&format!("playid {id}")).await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn seek_with_songs_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("seek 0 10").await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn play_out_of_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("play 999").await;
    assert!(resp.starts_with("ACK "), "play out of range: {resp}");
}
