//! Tests for MPD option commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn setvol_valid() {
    let (_server, mut client) = setup().await;
    let resp = client.command("setvol 50").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "volume"), Some("50"));
}

#[tokio::test]
async fn setvol_boundary_values() {
    let (_server, mut client) = setup().await;

    let resp = client.command("setvol 0").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "volume"), Some("0"));

    let resp = client.command("setvol 100").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "volume"), Some("100"));
}

#[tokio::test]
async fn setvol_out_of_range() {
    let (_server, mut client) = setup().await;
    let resp = client.command("setvol 999").await;
    assert!(resp.starts_with("ACK "), "setvol 999 should error: {resp}");
}

#[tokio::test]
async fn getvol_returns_volume() {
    let (_server, mut client) = setup().await;
    client.command("setvol 42").await;
    let resp = client.command("getvol").await;
    assert_ok(&resp);
    assert_eq!(get_field(&resp, "volume"), Some("42"));
}

#[tokio::test]
async fn volume_relative_change() {
    let (_server, mut client) = setup().await;
    client.command("setvol 50").await;
    let resp = client.command("volume 10").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "volume"), Some("60"));
}

#[tokio::test]
async fn repeat_toggle() {
    let (_server, mut client) = setup().await;

    let resp = client.command("repeat 1").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "repeat"), Some("1"));

    let resp = client.command("repeat 0").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "repeat"), Some("0"));
}

#[tokio::test]
async fn random_toggle() {
    let (_server, mut client) = setup().await;

    let resp = client.command("random 1").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "random"), Some("1"));

    let resp = client.command("random 0").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "random"), Some("0"));
}

#[tokio::test]
async fn single_modes() {
    let (_server, mut client) = setup().await;

    for mode in &["0", "1", "oneshot"] {
        let resp = client.command(&format!("single {mode}")).await;
        assert_ok(&resp);
        let status = client.command("status").await;
        assert_eq!(get_field(&status, "single"), Some(*mode));
    }
}

#[tokio::test]
async fn consume_modes() {
    let (_server, mut client) = setup().await;

    for mode in &["0", "1", "oneshot"] {
        let resp = client.command(&format!("consume {mode}")).await;
        assert_ok(&resp);
        let status = client.command("status").await;
        assert_eq!(get_field(&status, "consume"), Some(*mode));
    }
}

#[tokio::test]
async fn crossfade_set() {
    let (_server, mut client) = setup().await;
    let resp = client.command("crossfade 5").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "xfade"), Some("5"));
}

#[tokio::test]
async fn replay_gain_mode() {
    let (_server, mut client) = setup().await;
    for mode in ["off", "track", "album", "auto"] {
        let resp = client.command(&format!("replay_gain_mode {mode}")).await;
        assert_ok(&resp);
    }
    let resp = client.command("replay_gain_status").await;
    assert_ok(&resp);
    assert!(get_field(&resp, "replay_gain_mode").is_some());
}
