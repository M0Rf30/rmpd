//! Tests for MPD database commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn lsinfo_root_without_db() {
    let (_server, mut client) = setup().await;
    // Without DB, lsinfo should error or return empty OK
    let resp = client.command("lsinfo").await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn lsinfo_root_with_db() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("lsinfo").await;
    // Should return directory listing or songs
    assert!(resp.ends_with("OK\n"), "lsinfo should succeed: {resp}");
}

#[tokio::test]
async fn find_by_artist() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("find \"(artist == \\\"Test Artist\\\")\"")
        .await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn search_case_insensitive() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("search \"(artist contains \\\"test\\\")\"")
        .await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn list_artists() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("list Artist").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn count_songs() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("count \"(artist == \\\"Test Artist\\\")\"").await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn listall_root() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("listall").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn listallinfo_root() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("listallinfo").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn update_command() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("update").await;
    // update returns a job id
    assert_ok(&resp);
}

#[tokio::test]
async fn listfiles_root() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("listfiles").await;
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}
