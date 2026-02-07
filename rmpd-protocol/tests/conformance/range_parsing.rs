//! Tests for MPD range parsing in commands that accept ranges.
//! Inspired by MPD's test_protocol.cxx range parsing tests.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn playlistinfo_single_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // Single position: should return one song (MPD treats N as N:N+1)
    let resp = client.command("playlistinfo 1").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(
        file_count, 1,
        "single position should return 1 song: {resp}"
    );
}

#[tokio::test]
async fn playlistinfo_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // Range 0:2 should return songs at positions 0 and 1
    let resp = client.command("playlistinfo 0:2").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 2, "range 0:2 should return 2 songs: {resp}");
}

#[tokio::test]
async fn playlistinfo_open_ended_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // Open-ended range 1: should return songs from position 1 onwards
    let resp = client.command("playlistinfo 1:").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 2, "range 1: should return 2 songs: {resp}");
}

#[tokio::test]
async fn playlistinfo_full_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    // No range = all songs
    let resp = client.command("playlistinfo").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 2, "no range should return all songs: {resp}");
}

#[tokio::test]
async fn playlistinfo_range_0_to_1() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("playlistinfo 0:1").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 1, "range 0:1 should return 1 song: {resp}");
}

#[tokio::test]
async fn playlistinfo_range_entire_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("playlistinfo 0:3").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 3, "range 0:3 should return all 3 songs: {resp}");
}

#[tokio::test]
async fn playlistinfo_out_of_bounds() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    // Single position beyond queue should return empty (apply_range returns &[])
    let resp = client.command("playlistinfo 999").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 0, "out of bounds should return 0 songs");
}

#[tokio::test]
async fn delete_single_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("delete 1").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("2"));
}

#[tokio::test]
async fn delete_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // Delete range 0:2 should remove first two songs
    let resp = client.command("delete 0:2").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("1"));
}

#[tokio::test]
async fn shuffle_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("shuffle 0:2").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("3"));
}

#[tokio::test]
async fn move_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("move 0:2 2").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("3"));
}
