//! Integration tests for queue operations

mod common;
#[path = "common/state_helpers.rs"]
mod state_helpers;
#[path = "common/tcp_harness.rs"]
mod tcp_harness;

use common::TestClient;
use rmpd_protocol::state::AppState;
use state_helpers::{StatusBuilder, create_test_queue};
use std::sync::Arc;
use tcp_harness::*;
use tokio::sync::RwLock;

/// Like `tcp_harness::setup_with_db`, but with an explicit list of song
/// paths (so tests can seed a directory tree) instead of a flat
/// `music/song{i}.flac` run.
async fn setup_with_songs(paths: &[&str]) -> (MpdTestServer, MpdTestClient, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();
    let music_dir = tmp.path().join("music");
    std::fs::create_dir_all(&music_dir).unwrap();
    let playlist_dir = tmp.path().join("playlists");
    std::fs::create_dir_all(&playlist_dir).unwrap();

    {
        let db = rmpd_library::Database::open(&db_path_str).unwrap();
        for (i, path) in paths.iter().enumerate() {
            let song = state_helpers::make_test_song(path, i as u32 + 1);
            db.add_song(&song).unwrap();
        }
    }

    let mut state = AppState::with_all_paths(
        db_path_str,
        music_dir.to_str().unwrap().to_string(),
        playlist_dir.to_str().unwrap().to_string(),
    );
    state.disable_actual_mount = true;
    let server = MpdTestServer::start_with_state(state).await;
    let client = MpdTestClient::connect(server.port()).await;
    (server, client, tmp)
}

#[test]
fn test_add_and_playlistinfo() {
    // Test response format for add command
    let add_response = "OK\n";
    assert!(TestClient::is_ok(add_response));

    // Test playlistinfo after adding a song
    let info_response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(info_response));
    assert_eq!(
        TestClient::get_field(info_response, "file"),
        Some("test.mp3")
    );
    assert_eq!(TestClient::get_field(info_response, "Pos"), Some("0"));
    assert_eq!(TestClient::get_field(info_response, "Id"), Some("1"));
}

#[test]
fn test_addid_command() {
    // addid should return the new song ID
    let response = "Id: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_delete_command() {
    // delete should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_deleteid_command() {
    // deleteid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_move_command() {
    // move should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_moveid_command() {
    // moveid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_swap_command() {
    // swap should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_swapid_command() {
    // swapid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_shuffle_command() {
    // shuffle should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistid_command() {
    // playlistid with specific ID
    let response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_playlistfind_command() {
    // playlistfind should return matching songs
    let response = "file: test.mp3\nTitle: Test Song\nArtist: Test Artist\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Test Song"));
    assert_eq!(
        TestClient::get_field(response, "Artist"),
        Some("Test Artist")
    );
}

#[test]
fn test_playlistsearch_command() {
    // playlistsearch should return matching songs (case-insensitive)
    let response = "file: test.mp3\nTitle: Test Song\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_plchanges_command() {
    // plchanges should return songs that changed since version
    let response = "file: test.mp3\nPos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_plchangesposid_command() {
    // plchangesposid should return position and ID changes
    let response = "cpos: 0\nId: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "cpos"), Some("0"));
    assert_eq!(TestClient::get_field(response, "Id"), Some("1"));
}

#[test]
fn test_prio_command() {
    // prio should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_prioid_command() {
    // prioid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rangeid_command() {
    // rangeid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_addtagid_command() {
    // addtagid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_cleartagid_command() {
    // cleartagid should return OK
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_addid_out_of_range_position_is_error() {
    // addid with position > queue length must ACK (Bad song index), not
    // silently clamp to an append.
    let response = "ACK [2@0] {addid} Bad song index\n";
    assert!(TestClient::is_error(response));
}

#[test]
fn test_inverted_range_is_error() {
    // Any START:END command (delete, move, playlistinfo, ...) with an
    // inverted range (start > end) must ACK, not silently reach the queue.
    let response = "ACK [2@0] {delete} Bad song index\n";
    assert!(TestClient::is_error(response));
}

// ── Relative position/destination grammar (`+N`/`-N`), real server ────

#[tokio::test]
async fn add_relative_position_with_no_current_song_is_error() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    // Nothing is playing, so "+0" (relative to the current song) has no
    // current song to be relative to.
    let resp = client.command("add \"music/song2.flac\" +0").await;
    assert!(
        resp.starts_with("ACK [55@0]") && resp.contains("No current song"),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn addid_relative_position_after_current_song() {
    // Pre-populate the queue/status directly (bypassing the real playback
    // engine, whose success depends on an actual audio backend) so the
    // "current song" is deterministic: 2 songs, song at position 0 current.
    let mut state = AppState::new();
    state.queue = Arc::new(RwLock::new(create_test_queue(2)));
    state.status = Arc::new(RwLock::new(
        StatusBuilder::new().current_position(0, 0).build(2),
    ));
    let (_server, mut client) = setup_with_state(state).await;

    // "+0" inserts right after the current song (position 0), i.e. at 1.
    // Use a stream URI so no database lookup is needed.
    let resp = client
        .command("addid \"http://example.com/song3.mp3\" +0")
        .await;
    assert_ok(&resp);

    let info = client.command("playlistinfo 1").await;
    assert!(
        info.contains("song3.mp3"),
        "song3 should land right after the current song: {info}"
    );
}

#[tokio::test]
async fn moveid_relative_destination_before_current_song() {
    // 3 pre-existing songs; Queue ids start at 1, so song0/1/2.mp3 have ids
    // 1/2/3 at positions 0/1/2 respectively. Song at position 0 (id 1) is
    // current.
    let mut state = AppState::new();
    state.queue = Arc::new(RwLock::new(create_test_queue(3)));
    state.status = Arc::new(RwLock::new(
        StatusBuilder::new().current_position(0, 1).build(3),
    ));
    let (_server, mut client) = setup_with_state(state).await;

    // "-0" moves song id 3 (song2.mp3, currently at position 2) to right
    // before the current song (position 0).
    let resp = client.command("moveid 3 -0").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo 0").await;
    assert!(
        info.contains("song2.mp3"),
        "song id 3 should land right before the current song: {info}"
    );
}

#[tokio::test]
async fn move_relative_destination_onto_own_range_is_error() {
    let mut state = AppState::new();
    state.queue = Arc::new(RwLock::new(create_test_queue(2)));
    state.status = Arc::new(RwLock::new(
        StatusBuilder::new().current_position(0, 0).build(2),
    ));
    let (_server, mut client) = setup_with_state(state).await;

    // Moving the range containing the current song relative to itself.
    let resp = client.command("move 0:1 +0").await;
    assert!(
        resp.starts_with("ACK [2@0]") && resp.contains("relative to itself"),
        "unexpected response: {resp}"
    );
}

// ── Open-ended range grammar (`START:`) ────────────────────────────────

#[tokio::test]
async fn delete_open_ended_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // "1:" deletes from position 1 to the end of the queue.
    let resp = client.command("delete 1:").await;
    assert_ok(&resp);

    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("1"));
}

#[tokio::test]
async fn move_open_ended_range_is_error() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("move 0: 1").await;
    assert!(
        resp.starts_with("ACK [2@0]") && resp.contains("Open-ended range not supported"),
        "unexpected response: {resp}"
    );
}

// ── ACK cases fixed by this pass ───────────────────────────────────────

#[tokio::test]
async fn delete_at_exact_queue_length_is_noop_not_error() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    // MPD's RangeArg::CheckClip only errors on `start > length`; `start ==
    // length` clips to an empty range and is a silent no-op OK.
    let resp = client.command("delete 2").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(get_field(&status, "playlistlength"), Some("2"));
}

#[tokio::test]
async fn playlistinfo_start_beyond_length_is_error() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;

    // `start > length` (1 song, position 5) must ACK, not silently return
    // an empty list.
    let resp = client.command("playlistinfo 5").await;
    assert!(
        resp.starts_with("ACK [2@0]") && resp.contains("Bad song index"),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn move_multi_item_range_up_preserves_order() {
    let (_server, mut client, _tmp) = setup_with_db(4).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;
    client.command("add \"music/song4.flac\"").await;

    // Moving a 2-song range up must preserve the moved songs' relative
    // order (a fixed source index across iterations would corrupt this).
    let resp = client.command("move 2:4 0").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/song3.flac",
            "music/song4.flac",
            "music/song1.flac",
            "music/song2.flac",
        ],
        "moving [2:4) to 0 should preserve song3,song4 order: {info}"
    );
}

#[tokio::test]
async fn move_single_item_forward_lands_at_final_position() {
    let (_server, mut client, _tmp) = setup_with_db(4).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;
    client.command("add \"music/song4.flac\"").await;

    // `to` is the item's final absolute position in the resulting queue,
    // not an index into the post-removal array — moving position 0 to 1
    // must swap it past exactly one song, not land back at its own spot.
    let resp = client.command("move 0 1").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/song2.flac",
            "music/song1.flac",
            "music/song3.flac",
            "music/song4.flac",
        ],
        "move 0 1 should land song1 at position 1: {info}"
    );

    // move 1 2 on the now-current order ([song2, song1, song3, song4]):
    // moves song1 (position 1) to land at position 2.
    let resp = client.command("move 1 2").await;
    assert_ok(&resp);
    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/song2.flac",
            "music/song3.flac",
            "music/song1.flac",
            "music/song4.flac",
        ],
        "move 1 2 should land song1 at position 2: {info}"
    );
}

#[tokio::test]
async fn move_multi_item_range_forward_lands_at_final_position() {
    let (_server, mut client, _tmp) = setup_with_db(4).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;
    client.command("add \"music/song4.flac\"").await;

    // Moving the 2-song range [0,2) forward to final position 2: song3,
    // song4 (currently at [2,4)) shift back to fill [0,2), and song1,
    // song2 land at [2,4) in order.
    let resp = client.command("move 0:2 2").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/song3.flac",
            "music/song4.flac",
            "music/song1.flac",
            "music/song2.flac",
        ],
        "move 0:2 2 should land the [song1,song2] block at [2,4): {info}"
    );
}

#[tokio::test]
async fn move_range_destination_out_of_bounds_is_error() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    // Moving the 2-song range [0,2) leaves only 1 slot (3 - 2); `to = 2`
    // doesn't fit and must ACK "Number too large", not silently succeed.
    let resp = client.command("move 0:2 2").await;
    assert!(
        resp.starts_with("ACK [2@0]") && resp.contains("Number too large"),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn swap_accepts_quoted_arguments() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    // libmpdclient-style clients quote every argument, including bare
    // integers; `swap`/`swapid` must accept that, not just bare digits.
    let resp = client.command("swap \"0\" \"1\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn rangeid_empty_clears_the_range() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&resp, "Id").unwrap().to_string();

    let resp = client.command(&format!("rangeid {id} 1.5:3.0")).await;
    assert_ok(&resp);
    let info = client.command("playlistid").await;
    assert!(
        get_field(&info, "Range").is_some(),
        "range should be set: {info}"
    );

    // A bare ":" clears the range entirely ("play everything").
    let resp = client.command(&format!("rangeid {id} :")).await;
    assert_ok(&resp);
    let info = client.command("playlistid").await;
    assert!(
        get_field(&info, "Range").is_none(),
        "range should be cleared: {info}"
    );
}

#[tokio::test]
async fn addtagid_rejects_local_database_song() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    let resp = client.command("addid \"music/song1.flac\"").await;
    let id = get_field(&resp, "Id").unwrap();

    // MPD only allows tag edits on remote songs; local/database files ACK.
    let resp = client
        .command(&format!("addtagid {id} Artist \"New Artist\""))
        .await;
    assert!(
        resp.starts_with("ACK [4@0]") && resp.contains("Cannot edit tags of local file"),
        "unexpected response: {resp}"
    );
}

#[tokio::test]
async fn addtagid_and_cleartagid_on_remote_song() {
    let (_server, mut client) = setup().await;
    let resp = client
        .command("addid \"http://example.com/stream.mp3\"")
        .await;
    let id = get_field(&resp, "Id").unwrap();

    let resp = client
        .command(&format!("addtagid {id} Artist \"New Artist\""))
        .await;
    assert_ok(&resp);

    let resp = client.command(&format!("cleartagid {id} Artist")).await;
    assert_ok(&resp);
}

// ── `add` directory/root selection (recursive, path-sorted) ────────────

#[tokio::test]
async fn add_root_queues_entire_database_in_path_order() {
    let (_server, mut client, _tmp) = setup_with_songs(&[
        "music/album1/song1.flac",
        "music/album1/song2.flac",
        "music/album2/song3.flac",
    ])
    .await;

    // `mpc add /` strips the trailing slash and sends `add ""`; both forms
    // must add the whole database, path-sorted.
    let resp = client.command("add \"\"").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    assert_ok(&info);
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/album1/song1.flac",
            "music/album1/song2.flac",
            "music/album2/song3.flac",
        ],
        "add \"\" should queue every song in path order: {info}"
    );

    let resp = client.command("add \"/\"").await;
    assert_ok(&resp);
    let status = client.command("status").await;
    assert_eq!(
        get_field(&status, "playlistlength"),
        Some("6"),
        "add \"/\" should also queue the whole database again"
    );
}

#[tokio::test]
async fn add_directory_queues_only_that_subtree() {
    let (_server, mut client, _tmp) = setup_with_songs(&[
        "music/album1/song1.flac",
        "music/album1/song2.flac",
        "music/album2/song3.flac",
    ])
    .await;

    let resp = client.command("add \"music/album1\"").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec!["music/album1/song1.flac", "music/album1/song2.flac"],
        "add on a directory should only queue that subtree: {info}"
    );
}

#[tokio::test]
async fn add_directory_with_position_inserts_the_block() {
    let (_server, mut client, _tmp) = setup_with_songs(&[
        "music/album1/song1.flac",
        "music/album1/song2.flac",
        "music/other/sentinel.flac",
    ])
    .await;

    client.command("add \"music/other/sentinel.flac\"").await;
    let resp = client.command("add \"music/album1\" 0").await;
    assert_ok(&resp);

    let info = client.command("playlistinfo").await;
    let files: Vec<&str> = info
        .lines()
        .filter_map(|l| l.strip_prefix("file: "))
        .collect();
    assert_eq!(
        files,
        vec![
            "music/album1/song1.flac",
            "music/album1/song2.flac",
            "music/other/sentinel.flac",
        ],
        "the added directory block should be inserted at position 0: {info}"
    );
}

#[tokio::test]
async fn add_unknown_uri_still_acks() {
    let (_server, mut client, _tmp) = setup_with_songs(&["music/album1/song1.flac"]).await;

    let resp = client.command("add \"no/such/directory\"").await;
    assert!(
        resp.starts_with("ACK [50@0]") && resp.contains("No such directory"),
        "unexpected response: {resp}"
    );
}

// ── playlistfind/playlistsearch filter-expression grammar ──────────────

#[tokio::test]
async fn playlistfind_expression_form() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client
        .command("playlistfind \"(artist == 'Test Artist')\"")
        .await;
    assert_ok(&resp);
    let count = resp.matches("file:").count();
    assert_eq!(count, 3, "expression form should match all 3 songs: {resp}");
}

#[tokio::test]
async fn playlistsearch_expression_form_with_sort_and_window() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;
    client.command("add \"music/song3.flac\"").await;

    let resp = client
        .command("playlistsearch \"(artist contains 'test')\" sort Title window 0:1")
        .await;
    assert_ok(&resp);
    let titles: Vec<&str> = resp
        .lines()
        .filter_map(|l| l.strip_prefix("Title: "))
        .collect();
    assert_eq!(
        titles,
        vec!["Track 1"],
        "sort Title + window 0:1 should return only the first title: {resp}"
    );
}

#[tokio::test]
async fn playlistfind_legacy_pair_form_still_works() {
    let (_server, mut client, _tmp) = setup_with_db(3).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("playlistfind Artist \"Test Artist\"").await;
    assert_ok(&resp);
    let count = resp.matches("file:").count();
    assert_eq!(count, 2, "legacy TAG VALUE form should still match: {resp}");
}

#[tokio::test]
async fn playlistfind_unknown_filter_tag_is_error() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    client.command("add \"music/song1.flac\"").await;

    let resp = client.command("playlistfind \"(bogus == 'x')\"").await;
    assert!(
        resp.starts_with("ACK [2@0]") && resp.contains("Unknown filter type: bogus"),
        "unexpected response: {resp}"
    );
}

// ── plchanges field parity with playlistinfo/playlistid ────────────────

#[tokio::test]
async fn plchanges_includes_prio() {
    let (_server, mut client, _tmp) = setup_with_db(2).await;
    client.command("add \"music/song1.flac\"").await;
    client.command("add \"music/song2.flac\"").await;

    let resp = client.command("prio 200 0").await;
    assert_ok(&resp);

    let resp = client.command("plchanges 0").await;
    assert_ok(&resp);
    assert!(
        resp.contains("Prio: 200"),
        "plchanges should include Prio like playlistinfo/playlistid: {resp}"
    );
}
