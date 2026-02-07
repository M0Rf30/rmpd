//! Extended stored playlist conformance tests.
//! Tests save modes, load with range/position, searchplaylist, playlistlength.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn save_default_replaces_existing() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("save \"dup\"").await;
    assert_ok(&resp);

    // MPD default: save replaces existing playlist (not create-or-fail)
    let resp = client.command("save \"dup\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn save_create_fails_on_existing() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"dup\" create").await;

    // Explicit create mode should fail on existing
    let resp = client.command("save \"dup\" create").await;
    assert!(
        resp.starts_with("ACK "),
        "save create on existing playlist should ACK: {resp}"
    );
}

#[tokio::test]
async fn save_replace_overwrites() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"rep\"").await;

    // Explicit replace mode also works
    client.command("add \"music/song2.flac\"").await;
    let resp = client.command("save \"rep\" replace").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn save_append_to_existing() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"app\"").await;

    // Add more songs to queue and save with append mode
    client.command("add \"music/song2.flac\"").await;
    let resp = client.command("save \"app\" append").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn load_appends_to_queue() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // Create a playlist with 1 song
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"applist\"").await;

    // Clear queue, add a different song, then load — should append
    client.command("clear").await;
    client.command("add \"music/song2.flac\"").await;
    let resp = client.command("load \"applist\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("2"),
        "load should append to queue, not replace"
    );
}

#[tokio::test]
async fn load_with_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // Create a playlist with 2 songs
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("save \"rangelist\"").await;

    // Clear queue and load with range 0:1 (just first song)
    client.command("clear").await;
    let resp = client.command("load \"rangelist\" 0:1").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("1"),
        "load with range 0:1 should add exactly 1 song"
    );
}

#[tokio::test]
async fn load_with_position() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // Create a playlist with 2 songs
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("save \"poslist\"").await;

    // Clear and add a different song, then load at position 1
    client.command("clear").await;
    client.command("add \"music/song3.flac\"").await;
    let resp = client.command("load \"poslist\" 0:2 1").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("3"),
        "queue should have 3 songs (1 existing + 2 loaded)"
    );
}

#[tokio::test]
async fn searchplaylist_finds_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"sp\"").await;

    let resp = client
        .command("searchplaylist \"sp\" Artist \"Test\"")
        .await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "file").is_some(),
        "searchplaylist should return matching songs: {resp}"
    );
}

#[tokio::test]
async fn playlistlength_returns_count() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("save \"pl\"").await;

    let resp = client.command("playlistlength \"pl\"").await;
    assert_ok(&resp);
    assert!(
        get_field(&resp, "songs").is_some(),
        "playlistlength must return songs field: {resp}"
    );
    assert!(
        get_field(&resp, "playtime").is_some(),
        "playlistlength must return playtime field: {resp}"
    );
}
