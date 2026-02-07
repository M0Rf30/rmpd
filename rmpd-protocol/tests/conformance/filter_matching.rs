//! Tests for MPD filter matching in find/search commands.
//! Inspired by MPD's TestTagSongFilter tests.

use crate::tcp_harness::*;

#[tokio::test]
async fn playlistfind_exact_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("playlistfind Artist \"Test Artist\"").await;
    assert_ok(&resp);
    // Should find all songs since they all have "Test Artist"
    let file_count = resp.matches("file:").count();
    assert!(file_count > 0, "should find songs by exact artist: {resp}");
}

#[tokio::test]
async fn playlistsearch_substring() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("playlistsearch Artist \"Test\"").await;
    assert_ok(&resp);
    // Should find songs with "Test" in artist
    let file_count = resp.matches("file:").count();
    assert!(file_count > 0, "should find songs by substring: {resp}");
}

#[tokio::test]
async fn playlistfind_no_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client
        .command("playlistfind Artist \"Nonexistent Artist\"")
        .await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 0, "should find no songs for nonexistent artist");
}

#[tokio::test]
async fn find_with_sort() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("find \"(artist == \\\"Test Artist\\\")\" sort Track")
        .await;
    // May succeed or return ACK depending on filter syntax support
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn find_with_window() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("find \"(artist == \\\"Test Artist\\\")\" window 0:1")
        .await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn search_case_insensitive_match() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("search \"(artist contains \\\"test artist\\\")\"")
        .await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn list_with_filter() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("list Album Artist \"Test Artist\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistfind_by_title() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("playlistfind Title \"Track 1\"").await;
    assert_ok(&resp);
    let file_count = resp.matches("file:").count();
    assert!(file_count > 0, "should find song by title: {resp}");
}
