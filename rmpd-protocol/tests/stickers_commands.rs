//! Integration tests for sticker commands

#[path = "common/tcp_harness.rs"]
mod tcp_harness;
use tcp_harness::*;

#[tokio::test]
async fn test_sticker_get_set_list_delete_roundtrip() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let uri = "music/song1.flac";

    assert_ok(
        &client
            .command(&format!("sticker set song \"{uri}\" rating 5"))
            .await,
    );

    let get = client
        .command(&format!("sticker get song \"{uri}\" rating"))
        .await;
    assert_eq!(get, "sticker: rating=5\nOK\n");

    let list = client
        .command(&format!("sticker list song \"{uri}\""))
        .await;
    assert!(list.contains("sticker: rating=5"), "got: {list}");

    assert_ok(
        &client
            .command(&format!("sticker delete song \"{uri}\" rating"))
            .await,
    );
    let after = client
        .command(&format!("sticker get song \"{uri}\" rating"))
        .await;
    assert!(after.starts_with("ACK"), "got: {after}");
}

#[tokio::test]
async fn test_sticker_get_not_found() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker get song \"music/song1.flac\" nosuch")
        .await;
    assert!(response.starts_with("ACK [50@0]"), "got: {response}");
}

#[tokio::test]
async fn test_sticker_set_empty_name_rejected() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker set song \"music/song1.flac\" \"\" \"x\"")
        .await;
    assert!(response.starts_with("ACK"), "got: {response}");
    assert!(response.contains("empty sticker name"), "got: {response}");
}

#[tokio::test]
async fn test_sticker_inc_dec_command() {
    // MPD's Inc/Dec never print the new value: just OK.
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let uri = "music/song1.flac";
    let inc = client
        .command(&format!("sticker inc song \"{uri}\" plays 3"))
        .await;
    assert_eq!(inc, "OK\n");
    let dec = client
        .command(&format!("sticker dec song \"{uri}\" plays 1"))
        .await;
    assert_eq!(dec, "OK\n");

    let get = client
        .command(&format!("sticker get song \"{uri}\" plays"))
        .await;
    assert_eq!(get, "sticker: plays=2\nOK\n");
}

#[tokio::test]
async fn test_sticker_inc_missing_delta_is_bad_request() {
    // The delta argument is mandatory in MPD; omitting it isn't "increment
    // by 1" (rmpd used to default it) — it's the same "bad request" a valid
    // domain with an unrecognized subcommand gets.
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker inc song \"music/song1.flac\" plays")
        .await;
    assert_eq!(response, "ACK [2@0] {sticker} bad request\n");
}

#[tokio::test]
async fn test_sticker_delete_all_with_nothing_to_delete_is_ok() {
    // Real MPD crashes here (StickerCommands.cxx formats a nullptr with
    // FmtError when the unnamed delete removes zero rows) — a genuine
    // upstream bug, not a spec to match. rmpd returns OK instead.
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker delete song \"music/song1.flac\"")
        .await;
    assert_ok(&response);
}

#[tokio::test]
async fn test_sticker_unrecognized_subcommand_validates_domain_first() {
    // MPD's handle_sticker resolves the domain (2nd token) before it ever
    // looks at the subcommand (1st token), so an invalid domain still wins
    // over an unrecognized subcommand.
    let (_server, mut client, _tmp) = setup_with_db(1).await;

    let response = client.command("sticker bogusop bogusdomain uri").await;
    assert_eq!(
        response,
        "ACK [2@0] {sticker} unknown sticker domain \"bogusdomain\"\n"
    );

    let response = client
        .command("sticker bogusop song \"music/song1.flac\"")
        .await;
    assert_eq!(response, "ACK [2@0] {sticker} bad request\n");
}

#[tokio::test]
async fn test_sticker_unsupported_domain_rejected() {
    // rmpd's sticker table only backs the `song` domain; other MPD 0.24
    // domains must fail loudly instead of misreading the domain as a URI.
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker get playlist \"myplaylist\" rating")
        .await;
    assert!(response.starts_with("ACK [2@0]"), "got: {response}");

    let response = client.command("sticker get bogus \"x\" rating").await;
    assert!(response.starts_with("ACK [2@0]"), "got: {response}");
    assert!(
        response.contains("unknown sticker domain"),
        "got: {response}"
    );
}

#[tokio::test]
async fn test_sticker_find_with_equals_operator() {
    let (_server, mut client, _tmp) = setup_with_db(2).await;
    assert_ok(
        &client
            .command("sticker set song \"music/song1.flac\" rating 5")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set song \"music/song2.flac\" rating 9")
            .await,
    );

    let response = client.command("sticker find song \"\" rating = 5").await;
    assert!(response.contains("music/song1.flac"), "got: {response}");
    assert!(!response.contains("music/song2.flac"), "got: {response}");
}

#[tokio::test]
async fn test_sticker_find_sort_and_window() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    assert_ok(
        &client
            .command("sticker set song \"music/song1.flac\" rating 5")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set song \"music/song2.flac\" rating 1")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set song \"music/song3.flac\" rating 9")
            .await,
    );

    // Sort ascending by numeric value, take just the middle one via window.
    let response = client
        .command("sticker find song \"\" rating sort value_int window \"1:2\"")
        .await;
    assert_ok(&response);
    assert!(response.contains("music/song1.flac"), "got: {response}");
    assert!(!response.contains("music/song2.flac"), "got: {response}");
    assert!(!response.contains("music/song3.flac"), "got: {response}");
}

#[tokio::test]
async fn test_sticker_find_unknown_sort_tag_rejected() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker find song \"\" rating sort bogus")
        .await;
    assert!(response.starts_with("ACK [2@0]"), "got: {response}");
}

#[tokio::test]
async fn test_stickernames_is_global_not_uri_scoped() {
    // `stickernames` takes no arguments and lists every distinct sticker
    // name across all songs (MPD's `SELECT DISTINCT name FROM sticker`).
    let (_server, mut client, _tmp) = setup_with_db(2).await;
    assert_ok(
        &client
            .command("sticker set song \"music/song1.flac\" rating 5")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set song \"music/song2.flac\" playcount 10")
            .await,
    );

    let response = client.command("stickernames").await;
    assert!(response.contains("name: playcount"), "got: {response}");
    assert!(response.contains("name: rating"), "got: {response}");
    assert!(!response.contains("sticker:"), "got: {response}");
}

#[tokio::test]
async fn test_stickernamestypes_lists_name_and_type_pairs() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    assert_ok(
        &client
            .command("sticker set song \"music/song1.flac\" rating 5")
            .await,
    );

    let response = client.command("stickernamestypes").await;
    assert!(
        response.contains("name: rating\ntype: song"),
        "got: {response}"
    );

    // A domain with no stored stickers legitimately yields an empty OK.
    let response = client.command("stickernamestypes playlist").await;
    assert_eq!(response, "OK\n");
}

#[tokio::test]
async fn test_sticker_types_command() {
    let (_server, mut client) = setup().await;
    let response = client.command("stickertypes").await;
    assert!(
        response.starts_with("stickertype: filter\n"),
        "got: {response}"
    );
    assert!(
        response.contains("stickertype: playlist\n"),
        "got: {response}"
    );
    assert!(response.contains("stickertype: song\n"), "got: {response}");
    assert_ok(&response);
}
