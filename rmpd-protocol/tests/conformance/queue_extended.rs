//! Extended queue command conformance tests.
//! Tests add/addid with position parameters.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn add_with_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    let resp = client.command("add \"music/song2.flac\" 0").await;
    assert_ok(&resp);

    // song2 should be at position 0
    let info = client.command("playlistinfo 0").await;
    assert_ok(&info);
    assert!(
        info.contains("song2.flac"),
        "song2 should be at position 0: {info}"
    );
}

#[tokio::test]
async fn addid_with_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    let resp = client.command("addid \"music/song2.flac\" 0").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "Id").is_some(),
        "addid must return Id field: {resp}"
    );

    // song2 should be at position 0
    let info = client.command("playlistinfo 0").await;
    assert_ok(&info);
    assert!(
        info.contains("song2.flac"),
        "song2 should be at position 0: {info}"
    );
}

#[tokio::test]
async fn add_with_position_out_of_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    // MPD clamps out-of-range positions; some implementations return ACK
    let resp = client.command("add \"music/song1.flac\" 999").await;
    // Either OK (clamped) or ACK is acceptable
    assert!(
        resp.ends_with("OK\n") || resp.starts_with("ACK "),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn addid_nonexistent_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"no_such.flac\"").await;
    assert!(
        resp.starts_with("ACK "),
        "addid nonexistent song should ACK: {resp}"
    );
}

#[tokio::test]
async fn add_returns_id() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("add \"music/song1.flac\"").await;
    assert_ok(&resp);
    // rmpd returns Id field from add (MPD 0.23+ compatible)
    assert!(
        get_field(&resp, "Id").is_some(),
        "add should return Id field: {resp}"
    );
}
