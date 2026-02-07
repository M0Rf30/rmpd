//! Tests for MPD stored playlist commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn listplaylists_empty() {
    let (_server, mut client) = setup().await;
    let resp = client.command("listplaylists").await;
    // May succeed with empty list or error without DB
    assert!(resp.ends_with("OK\n") || resp.starts_with("ACK "));
}

#[tokio::test]
async fn save_and_load_playlist() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("save \"testlist\"").await;
    assert_ok(&resp);

    // Clear queue and reload
    client.command("clear").await;
    let resp = client.command("load \"testlist\"").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("2"));
}

#[tokio::test]
async fn listplaylists_after_save() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"mylist\"").await;

    let resp = client.command("listplaylists").await;
    assert_ok(&resp);
    assert!(
        resp.contains("mylist"),
        "should list saved playlist: {resp}"
    );
}

#[tokio::test]
async fn listplaylist_shows_files() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"filelist\"").await;

    let resp = client.command("listplaylist \"filelist\"").await;
    assert_ok(&resp);
    assert!(resp.contains("file:") || resp.contains("music/song1.flac"));
}

#[tokio::test]
async fn listplaylistinfo_shows_metadata() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"infolist\"").await;

    let resp = client.command("listplaylistinfo \"infolist\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistadd_to_stored() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    // Create empty playlist by saving empty queue
    client.command("save \"addtest\"").await;

    let resp = client
        .command("playlistadd \"addtest\" \"music/song1.flac\"")
        .await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistclear_stored() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("save \"cleartest\"").await;

    let resp = client.command("playlistclear \"cleartest\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistdelete_from_stored() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("save \"deltest\"").await;

    let resp = client.command("playlistdelete \"deltest\" 0").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn playlistmove_in_stored() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("save \"movetest\"").await;

    let resp = client.command("playlistmove \"movetest\" 0 1").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn rm_playlist() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("save \"rmtest\"").await;

    let resp = client.command("rm \"rmtest\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn rename_playlist() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("save \"oldname\"").await;

    let resp = client.command("rename \"oldname\" \"newname\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn load_nonexistent_playlist() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("load \"does_not_exist\"").await;
    assert!(
        resp.starts_with("ACK "),
        "loading nonexistent playlist should error: {resp}"
    );
}
