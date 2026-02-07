//! Tests for MPD queue commands over TCP.

use crate::tcp_harness::*;

// ── Basic queue operations ───────────────────────────────────────────

#[tokio::test]
async fn clear_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("clear").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistinfo_empty_queue() {
    let (_server, mut client) = setup().await;
    let resp = client.command("playlistinfo").await;
    assert_ok(&resp);
    // Empty queue returns only "OK"
    assert_eq!(resp, "OK\n");
}

#[tokio::test]
async fn add_requires_db() {
    // Without a database configured, add should fail
    let (_server, mut client) = setup().await;
    let resp = client.command("add \"test.flac\"").await;
    assert!(
        resp.starts_with("ACK "),
        "add without db should fail: {resp}"
    );
}

#[tokio::test]
async fn addid_requires_db() {
    let (_server, mut client) = setup().await;
    let resp = client.command("addid \"test.flac\"").await;
    assert!(
        resp.starts_with("ACK "),
        "addid without db should fail: {resp}"
    );
}

// ── Queue operations with database ──────────────────────────────────

#[tokio::test]
async fn add_and_playlistinfo() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("add \"music/song1.flac\"").await;
    assert_ok(&resp);

    let resp = client.command("playlistinfo").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "file").is_some(),
        "queue should have a song"
    );
}

#[tokio::test]
async fn addid_returns_id() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "Id").is_some(),
        "addid must return Id field"
    );
}

#[tokio::test]
async fn delete_by_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("delete 0").await;
    assert_ok(&resp);

    // Should have 1 song left
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("1"));
}

#[tokio::test]
async fn deleteid_removes_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&resp, "Id").unwrap();

    let resp = client.command(&format!("deleteid {id}")).await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("0"));
}

#[tokio::test]
async fn move_song_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("move 0 2").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn moveid_song_in_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&resp, "Id").unwrap();
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command(&format!("moveid {id} 1")).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn swap_positions() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("swap 0 1").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn swapid_songs() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let r1 = client.command("addid \"music/song1.flac\"").await;
    let id1 = get_field(&r1, "Id").unwrap().to_string();
    let r2 = client.command("addid \"music/song2.flac\"").await;
    let id2 = get_field(&r2, "Id").unwrap().to_string();

    let resp = client.command(&format!("swapid {id1} {id2}")).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn shuffle_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client.command("shuffle").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistid_returns_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&resp, "Id").unwrap();

    let resp = client.command(&format!("playlistid {id}")).await;
    assert_ok(&resp);
    assert!(get_field(&resp, "file").is_some());
}

#[tokio::test]
async fn plchanges_returns_all_for_version_zero() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("plchanges 0").await;
    assert_ok(&resp);
    // Version 0 should return all songs
    let file_count = resp.matches("file:").count();
    assert_eq!(file_count, 2, "plchanges 0 should return all songs");
}

#[tokio::test]
async fn plchangesposid_returns_positions() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("plchangesposid 0").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "cpos").is_some()
            || get_field(&resp, "Pos").is_some()
            || resp.contains("Id:")
    );
}

#[tokio::test]
async fn prio_sets_priority() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("prio 10 0:1").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn prioid_sets_priority() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let r = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&r, "Id").unwrap();

    let resp = client.command(&format!("prioid 10 {id}")).await;
    assert_ok(&resp);
}

#[tokio::test]
async fn clear_removes_all_songs() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("clear").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("0"));
}

#[tokio::test]
async fn addtagid_and_cleartagid() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let r = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&r, "Id").unwrap();

    let resp = client
        .command(&format!("addtagid {id} Artist \"New Artist\""))
        .await;
    assert_ok(&resp);

    let resp = client.command(&format!("cleartagid {id} Artist")).await;
    assert_ok(&resp);
}
