// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

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
async fn test_sticker_delete_with_nothing_to_delete_is_no_exist() {
    // MPD master's DomainHandler::Delete reports `no stickers found` (an
    // unnamed delete) / `no such sticker` (a named one) with ACK_ERROR_NO_EXIST
    // when no row was removed.
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let response = client
        .command("sticker delete song \"music/song1.flac\"")
        .await;
    assert_eq!(
        response,
        "ACK [50@0] {sticker} no stickers found: \"music/song1.flac\"\n"
    );

    let response = client
        .command("sticker delete song \"music/song1.flac\" nosuch")
        .await;
    assert_eq!(
        response,
        "ACK [50@0] {sticker} no such sticker: \"nosuch\"\n"
    );
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
    // Matches MPD's handle_sticker_types (StickerCommands.cxx:504) exactly:
    // filter, playlist, song, then the allowed tag domains.
    assert!(
        response.starts_with("stickertype: filter\n"),
        "got: {response}"
    );
    assert!(
        response.contains("stickertype: playlist\n"),
        "got: {response}"
    );
    assert!(response.contains("stickertype: song\n"), "got: {response}");
    assert!(
        response.contains("stickertype: Artist\n"),
        "got: {response}"
    );
    assert_ok(&response);
}

// ── MPD 0.24 sticker domains: tag / playlist / filter ───────────────────

#[tokio::test]
async fn test_sticker_tag_domain_full_roundtrip() {
    let (_server, mut client, _tmp) = setup_with_db(2).await;

    assert_ok(
        &client
            .command("sticker set Album \"Test Album\" rating 8")
            .await,
    );
    // Tag names are case-insensitive on the command and stored canonically.
    assert_eq!(
        client
            .command("sticker get album \"Test Album\" rating")
            .await,
        "sticker: rating=8\nOK\n"
    );
    assert_eq!(
        client.command("sticker list ALBUM \"Test Album\"").await,
        "sticker: rating=8\nOK\n"
    );

    // inc/dec work and print nothing.
    assert_eq!(
        client
            .command("sticker inc Album \"Test Album\" rating 2")
            .await,
        "OK\n"
    );
    assert_eq!(
        client
            .command("sticker dec Album \"Test Album\" rating 3")
            .await,
        "OK\n"
    );
    assert_eq!(
        client
            .command("sticker get Album \"Test Album\" rating")
            .await,
        "sticker: rating=7\nOK\n"
    );

    // find prints `<canonical tag>: <value>`, not `file:`; empty URI = all.
    assert_eq!(
        client.command("sticker find album \"\" rating").await,
        "Album: Test Album\nsticker: rating=7\nOK\n"
    );
    // URI is a plain string prefix for non-song domains.
    assert_eq!(
        client.command("sticker find Album \"Test\" rating").await,
        "Album: Test Album\nsticker: rating=7\nOK\n"
    );
    assert_eq!(
        client.command("sticker find Album \"Other\" rating").await,
        "OK\n"
    );
    assert_eq!(
        client.command("sticker find Album \"\" rating = 7").await,
        "Album: Test Album\nsticker: rating=7\nOK\n"
    );
    assert_eq!(
        client.command("sticker find Album \"\" rating gt 7").await,
        "OK\n"
    );

    // Domains are isolated: the same URI/name under Artist or song is empty.
    assert!(
        client
            .command("sticker get Artist \"Test Album\" rating")
            .await
            .starts_with("ACK")
    );
    assert_eq!(
        client.command("sticker find song \"\" rating").await,
        "OK\n"
    );

    assert_eq!(
        client
            .command("sticker delete Album \"Test Album\" rating")
            .await,
        "OK\n"
    );
    assert_eq!(
        client
            .command("sticker get Album \"Test Album\" rating")
            .await,
        "ACK [50@0] {sticker} no such sticker: \"rating\"\n"
    );
}

#[tokio::test]
async fn test_sticker_tag_domain_errors() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;

    // The tag value must exist in the database.
    assert_eq!(
        client.command("sticker set Album \"Nope\" rating 1").await,
        "ACK [2@0] {sticker} no such Album: \"Nope\"\n"
    );
    // Error text uses the canonical tag spelling even for a lowercase domain.
    assert_eq!(
        client.command("sticker get artist \"Nope\" rating").await,
        "ACK [2@0] {sticker} no such Artist: \"Nope\"\n"
    );
    // A real tag that is not in the sticker allow-list.
    assert_eq!(
        client.command("sticker set Track \"1\" rating 1").await,
        "ACK [2@0] {sticker} unsupported tag: \"Track\"\n"
    );
    // `find` never validates, so a disallowed tag just finds nothing.
    assert_eq!(
        client.command("sticker find Track \"\" rating").await,
        "OK\n"
    );
    // Not a tag at all.
    assert_eq!(
        client.command("sticker get Bogus \"x\" rating").await,
        "ACK [2@0] {sticker} unknown sticker domain \"Bogus\"\n"
    );
    // Deleting where nothing is stored.
    assert_eq!(
        client.command("sticker delete Album \"Test Album\"").await,
        "ACK [50@0] {sticker} no stickers found: \"Test Album\"\n"
    );
    assert_eq!(
        client
            .command("sticker set Album \"Test Album\" \"\" 1")
            .await,
        "ACK [2@0] {sticker} empty sticker name\n"
    );
}

#[tokio::test]
async fn test_sticker_playlist_domain_full_roundtrip() {
    let (_server, mut client, tmp) = setup_with_db(1).await;
    let playlists = tmp.path().join("playlists");

    // Unknown playlist.
    assert_eq!(
        client
            .command("sticker set playlist \"mix\" rating 5")
            .await,
        "ACK [2@0] {sticker} no such playlist: \"mix\"\n"
    );

    std::fs::write(playlists.join("mix.m3u"), "music/song1.flac\n").unwrap();
    std::fs::write(playlists.join("mix2.m3u"), "").unwrap();

    assert_ok(
        &client
            .command("sticker set playlist \"mix\" rating 5")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set playlist \"mix2\" rating 9")
            .await,
    );
    assert_eq!(
        client.command("sticker get playlist \"mix\" rating").await,
        "sticker: rating=5\nOK\n"
    );
    assert_eq!(
        client.command("sticker list playlist \"mix\"").await,
        "sticker: rating=5\nOK\n"
    );
    assert_eq!(
        client.command("sticker inc playlist \"mix\" plays 4").await,
        "OK\n"
    );
    assert_eq!(
        client.command("sticker get playlist \"mix\" plays").await,
        "sticker: plays=4\nOK\n"
    );

    // find: `playlist: NAME` lines; empty URI = all playlists, else prefix.
    assert_eq!(
        client.command("sticker find playlist \"\" rating").await,
        "playlist: mix\nsticker: rating=5\nplaylist: mix2\nsticker: rating=9\nOK\n"
    );
    assert_eq!(
        client
            .command("sticker find playlist \"mix2\" rating")
            .await,
        "playlist: mix2\nsticker: rating=9\nOK\n"
    );
    assert_eq!(
        client
            .command("sticker find playlist \"\" rating > 6")
            .await,
        "playlist: mix2\nsticker: rating=9\nOK\n"
    );
    // song domain does not see playlist stickers.
    assert_eq!(
        client.command("sticker find song \"\" rating").await,
        "OK\n"
    );

    // `rm` of a stored playlist removes its stickers (Instance::OnPlaylistDeleted).
    assert_ok(&client.command("rm mix").await);
    assert_eq!(
        client.command("sticker find playlist \"\" rating").await,
        "playlist: mix2\nsticker: rating=9\nOK\n"
    );
    assert_eq!(
        client.command("sticker get playlist \"mix\" rating").await,
        "ACK [2@0] {sticker} no such playlist: \"mix\"\n"
    );

    assert_eq!(
        client
            .command("sticker delete playlist \"mix2\" rating")
            .await,
        "OK\n"
    );
    assert_eq!(
        client.command("sticker delete playlist \"mix2\"").await,
        "ACK [50@0] {sticker} no stickers found: \"mix2\"\n"
    );
}

#[tokio::test]
async fn test_sticker_filter_domain_full_roundtrip() {
    let (_server, mut client, _tmp) = setup_with_db(2).await;

    assert_ok(
        &client
            .command(
                "sticker set filter \"((Album == \\\"Test Album\\\") AND (Artist == \\\"Test Artist\\\"))\" rating 6",
            )
            .await,
    );
    // The URI is normalized: different spelling of the same filter hits the
    // same row (lower-case tag names, extra grouping, case/space variations).
    assert_eq!(
        client
            .command(
                "sticker get filter \"((album == \\\"Test Album\\\") AND ((artist == \\\"Test Artist\\\")))\" rating"
            )
            .await,
        "sticker: rating=6\nOK\n"
    );
    assert_eq!(
        client
            .command(
                "sticker list filter \"((Album == \\\"Test Album\\\") AND (Artist == \\\"Test Artist\\\"))\""
            )
            .await,
        "sticker: rating=6\nOK\n"
    );
    assert_eq!(
        client
            .command(
                "sticker inc filter \"((Album == \\\"Test Album\\\") AND (Artist == \\\"Test Artist\\\"))\" rating 1"
            )
            .await,
        "OK\n"
    );

    // find prints the normalized expression under `filter:`.
    assert_eq!(
        client.command("sticker find filter \"\" rating").await,
        "filter: ((Album == \"Test Album\") AND (Artist == \"Test Artist\"))\nsticker: rating=7\nOK\n"
    );

    // Single term is not wrapped in an extra group.
    assert_ok(
        &client
            .command("sticker set filter \"(Genre == \\\"Rock\\\")\" liked yes")
            .await,
    );
    assert_eq!(
        client.command("sticker find filter \"\" liked").await,
        "filter: (Genre == \"Rock\")\nsticker: liked=yes\nOK\n"
    );

    assert_eq!(
        client
            .command("sticker delete filter \"(Genre == \\\"Rock\\\")\" liked")
            .await,
        "OK\n"
    );
}

#[tokio::test]
async fn test_sticker_filter_domain_errors() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;

    // Valid filter, no song matches: `no matches found` with the normalized
    // expression (ACK_ERROR_ARG).
    assert_eq!(
        client
            .command("sticker set filter \"(album == \\\"Nope\\\")\" rating 1")
            .await,
        "ACK [2@0] {sticker} no matches found: \"(Album == \\\"Nope\\\")\"\n"
    );
    // Parse failures are runtime errors in MPD (ACK_ERROR_UNKNOWN).
    assert_eq!(
        client
            .command("sticker get filter \"not a filter\" rating")
            .await,
        "ACK [5@0] {sticker} Incorrect number of filter arguments\n"
    );
    let response = client
        .command("sticker get filter \"(bogus == \\\"x\\\")\" rating")
        .await;
    assert!(
        response.starts_with("ACK [5@0] {sticker} "),
        "got: {response}"
    );
    // `find` is unvalidated.
    assert_eq!(
        client.command("sticker find filter \"(\" rating").await,
        "OK\n"
    );
}

#[tokio::test]
async fn test_stickernamestypes_across_domains() {
    let (_server, mut client, tmp) = setup_with_db(1).await;
    std::fs::write(tmp.path().join("playlists/mix.m3u"), "").unwrap();

    assert_ok(
        &client
            .command("sticker set song \"music/song1.flac\" rating 5")
            .await,
    );
    assert_ok(
        &client
            .command("sticker set Album \"Test Album\" rating 5")
            .await,
    );
    assert_ok(&client.command("sticker set playlist \"mix\" fav 1").await);

    // Ordered by name; one entry per (name, type).
    assert_eq!(
        client.command("stickernamestypes").await,
        "name: fav\ntype: playlist\nname: rating\ntype: Album\nname: rating\ntype: song\nOK\n"
    );
    assert_eq!(
        client.command("stickernamestypes Album").await,
        "name: rating\ntype: Album\nOK\n"
    );
    assert_eq!(
        client.command("stickernamestypes playlist").await,
        "name: fav\ntype: playlist\nOK\n"
    );
    assert_eq!(client.command("stickernamestypes filter").await, "OK\n");
    assert_eq!(
        client.command("stickernamestypes Track").await,
        "ACK [2@0] {stickernamestypes} unsupported tag \"Track\"\n"
    );
    assert_eq!(
        client.command("stickernamestypes bogus").await,
        "ACK [2@0] {stickernamestypes} no such tag \"bogus\"\n"
    );

    // `stickernames` stays a flat unique list across all domains.
    assert_eq!(
        client.command("stickernames").await,
        "name: fav\nname: rating\nOK\n"
    );
}
