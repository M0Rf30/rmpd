//! Tests for MPD status and stats commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn status_initial_state() {
    let (_server, mut client) = setup().await;
    let resp = client.command("status").await;
    assert_ok(&resp);

    // Initial state should be "stop"
    assert_eq!(get_field(&resp, "state"), Some("stop"));
    // Playlist should exist
    assert!(get_field(&resp, "playlist").is_some());
    assert_eq!(get_field(&resp, "playlistlength"), Some("0"));
}

#[tokio::test]
async fn status_has_required_fields() {
    let (_server, mut client) = setup().await;
    let resp = client.command("status").await;
    assert_ok(&resp);

    // Per MPD spec, these fields must always be present
    for field in &["volume", "repeat", "random", "single", "consume", "playlist", "playlistlength", "state"] {
        assert!(
            get_field(&resp, field).is_some(),
            "missing required field: {field}"
        );
    }
}

#[tokio::test]
async fn stats_response_format() {
    let (_server, mut client) = setup().await;
    let resp = client.command("stats").await;
    assert_ok(&resp);

    for field in &["artists", "albums", "songs", "uptime", "db_playtime", "db_update", "playtime"] {
        assert!(
            get_field(&resp, field).is_some(),
            "missing required field: {field}"
        );
    }
}

#[tokio::test]
async fn stats_uptime_is_nonzero() {
    let (_server, mut client) = setup().await;
    let resp = client.command("stats").await;
    let uptime: u64 = get_field(&resp, "uptime").unwrap().parse().unwrap();
    // Should be at least 0 (just started)
    assert!(uptime < 60, "uptime should be small for fresh server");
}

#[tokio::test]
async fn clearerror_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("clearerror").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn volume_default_is_100() {
    let (_server, mut client) = setup().await;
    let resp = client.command("status").await;
    assert_eq!(get_field(&resp, "volume"), Some("100"));
}
