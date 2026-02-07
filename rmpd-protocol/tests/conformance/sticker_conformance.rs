//! Tests for MPD sticker commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn sticker_set_and_get() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    let resp = client
        .command("sticker set song \"music/song1.flac\" rating 5")
        .await;
    assert_ok(&resp);

    let resp = client
        .command("sticker get song \"music/song1.flac\" rating")
        .await;
    assert_ok(&resp);
    assert!(
        resp.contains("rating=5"),
        "should return sticker value: {resp}"
    );
}

#[tokio::test]
async fn sticker_delete() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client
        .command("sticker set song \"music/song1.flac\" rating 5")
        .await;

    let resp = client
        .command("sticker delete song \"music/song1.flac\" rating")
        .await;
    assert_ok(&resp);

    // After delete, get should error
    let resp = client
        .command("sticker get song \"music/song1.flac\" rating")
        .await;
    assert!(
        resp.starts_with("ACK "),
        "deleted sticker should not exist: {resp}"
    );
}

#[tokio::test]
async fn sticker_list() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client
        .command("sticker set song \"music/song1.flac\" rating 5")
        .await;
    client
        .command("sticker set song \"music/song1.flac\" comment \"nice\"")
        .await;

    let resp = client
        .command("sticker list song \"music/song1.flac\"")
        .await;
    assert_ok(&resp);
    assert!(resp.contains("rating=5"), "should list rating: {resp}");
    assert!(resp.contains("comment=nice"), "should list comment: {resp}");
}

#[tokio::test]
async fn sticker_find() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client
        .command("sticker set song \"music/song1.flac\" rating 5")
        .await;

    let resp = client.command("sticker find song \"\" rating").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn sticker_get_nonexistent() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client
        .command("sticker get song \"music/song1.flac\" nonexistent")
        .await;
    assert!(
        resp.starts_with("ACK "),
        "nonexistent sticker should error: {resp}"
    );
}

#[tokio::test]
async fn sticker_inc_and_dec() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client
        .command("sticker set song \"music/song1.flac\" playcount 10")
        .await;

    let resp = client
        .command("sticker inc song \"music/song1.flac\" playcount 1")
        .await;
    assert_ok(&resp);

    let resp = client
        .command("sticker get song \"music/song1.flac\" playcount")
        .await;
    assert_ok(&resp);
    assert!(resp.contains("playcount=11"), "inc should add: {resp}");

    let resp = client
        .command("sticker dec song \"music/song1.flac\" playcount 1")
        .await;
    assert_ok(&resp);
}

#[tokio::test]
async fn sticker_delete_all_for_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;

    client
        .command("sticker set song \"music/song1.flac\" rating 5")
        .await;
    client
        .command("sticker set song \"music/song1.flac\" comment test")
        .await;

    // Delete without specifying name removes all stickers
    let resp = client
        .command("sticker delete song \"music/song1.flac\"")
        .await;
    assert_ok(&resp);
}
