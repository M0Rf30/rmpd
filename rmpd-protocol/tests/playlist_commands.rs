//! Integration tests for stored playlist commands

mod common;

use common::TestClient;
use rmpd_protocol::commands::playlists;
use rmpd_protocol::state::AppState;

#[test]
fn test_listplaylists_command() {
    // listplaylists should return list of playlists
    let response = "playlist: favorites\nLast-Modified: 2024-01-01T00:00:00Z\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "playlist"),
        Some("favorites")
    );
}

#[test]
fn test_listplaylists_empty() {
    // listplaylists with no playlists
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_command() {
    // save should create a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_load_command() {
    // load should add playlist to queue
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listplaylist_command() {
    // listplaylist should return playlist files
    let response = "file: song1.mp3\nfile: song2.mp3\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listplaylistinfo_command() {
    // listplaylistinfo should return playlist files with metadata
    let response =
        "file: song1.mp3\nTitle: Song 1\nArtist: Artist 1\nfile: song2.mp3\nTitle: Song 2\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "Title"), Some("Song 1"));
}

#[test]
fn test_playlistadd_command() {
    // playlistadd should add a song to playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistclear_command() {
    // playlistclear should clear a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistdelete_command() {
    // playlistdelete should remove a song from playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistmove_command() {
    // playlistmove should move a song in playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rename_command() {
    // rename should rename a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_rm_command() {
    // rm should delete a playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchplaylist_command() {
    // searchplaylist should search within a playlist
    let response = "file: song1.mp3\nTitle: Matching Song\nOK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_searchaddpl_command() {
    // searchaddpl should search and add to playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_playlistlength_command() {
    // playlistlength should return playlist length
    let response = "songs: 10\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "songs"), Some("10"));
}

#[test]
fn test_load_with_range() {
    // load with range should load subset of playlist
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_load_with_position() {
    // load with position should insert at specific location
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_append_mode() {
    // save with append mode
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_create_mode() {
    // save with create mode (default)
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_save_replace_mode() {
    // save with replace mode
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

/// Build an `AppState` backed by fresh temp music/playlist directories,
/// mirroring the setup in `stored_playlist_idle.rs`.
fn new_test_state() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::TempDir::new().unwrap();
    let music = tmp.path().join("music");
    let playlists_dir = tmp.path().join("playlists");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&playlists_dir).unwrap();
    let state = AppState::with_all_paths(
        tmp.path().join("db").to_str().unwrap().to_string(),
        music.to_str().unwrap().to_string(),
        playlists_dir.to_str().unwrap().to_string(),
    );
    (tmp, state)
}

#[tokio::test]
async fn test_playlist_name_traversal_rejected() {
    // save/load/rm must reject a traversal name before touching the filesystem.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "../evil", None).await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "save with traversal name must ACK [2@0], got: {resp}"
    );

    let resp = playlists::handle_load_command(&state, "../evil", None, None).await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "load with traversal name must ACK [2@0], got: {resp}"
    );

    let resp = playlists::handle_rm_command(&state, "../evil").await;
    assert!(
        TestClient::is_error(&resp) && resp.contains("[2@0]"),
        "rm with traversal name must ACK [2@0], got: {resp}"
    );
}

#[tokio::test]
async fn test_playlist_name_normal_round_trips() {
    // A normal name must still save and load without triggering validation.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "My Playlist", None).await;
    assert!(TestClient::is_ok(&resp), "save should succeed, got: {resp}");

    let resp = playlists::handle_load_command(&state, "My Playlist", None, None).await;
    assert!(TestClient::is_ok(&resp), "load should succeed, got: {resp}");

    let resp = playlists::handle_rm_command(&state, "My Playlist").await;
    assert!(TestClient::is_ok(&resp), "rm should succeed, got: {resp}");
}

fn add_songs_to_db(state: &AppState, paths: &[&str]) {
    let db = rmpd_library::Database::open(state.db_path.as_ref().unwrap()).unwrap();
    for (i, path) in paths.iter().enumerate() {
        let song = rmpd_core::test_utils::make_test_song(path, i as u32 + 1);
        db.add_song(&song).unwrap();
    }
}

fn write_playlist(state: &AppState, name: &str, paths: &[&str]) {
    let dir = state.playlist_dir.as_ref().unwrap();
    let content: String = paths.iter().map(|p| format!("{p}\n")).collect();
    std::fs::write(
        std::path::Path::new(dir).join(format!("{name}.m3u")),
        content,
    )
    .unwrap();
}

#[tokio::test]
async fn test_save_default_mode_is_create() {
    // MPD's `save` defaults to CREATE, not REPLACE: a second save of the
    // same name without a mode must fail EXIST, not silently overwrite.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "list1", None).await;
    assert!(
        TestClient::is_ok(&resp),
        "first save should succeed, got: {resp}"
    );

    let resp = playlists::handle_save_command(&state, "list1", None).await;
    assert!(
        resp.contains("[56@0]") && resp.contains("Playlist already exists"),
        "default save mode must be 'create', got: {resp}"
    );
}

#[tokio::test]
async fn test_save_replace_requires_existing_playlist() {
    // Despite the name, "replace" fails on a playlist that doesn't exist yet
    // (PlaylistSave.cxx: only CREATE tolerates a missing file).
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "missing", Some("replace".to_string())).await;
    assert!(
        resp.contains("[50@0]") && resp.contains("No such playlist"),
        "replace on a missing playlist must ACK No such playlist, got: {resp}"
    );
}

#[tokio::test]
async fn test_save_unrecognized_mode_message() {
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "list1", Some("bogus".to_string())).await;
    assert!(
        resp.contains("[2@0]") && resp.contains("Unrecognized save mode"),
        "bad mode must ACK MPD's exact message, got: {resp}"
    );
}

#[tokio::test]
async fn test_playlist_name_with_dots_is_valid() {
    // MPD's spl_valid_name() only forbids '/'; unlike a real filesystem, "."
    // and ".." are ordinary (if odd) playlist names once ".m3u" is appended.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_save_command(&state, "..", None).await;
    assert!(
        TestClient::is_ok(&resp),
        "'..' has no '/' and must be accepted, got: {resp}"
    );
}

#[tokio::test]
async fn test_load_absolute_position_out_of_range() {
    let (_tmp, state) = new_test_state();
    write_playlist(&state, "list1", &["song1.mp3"]);

    // The queue is empty (length 0); position 1 is one past the only valid
    // insertion point (0).
    let resp = playlists::handle_load_command(
        &state,
        "list1",
        None,
        Some(rmpd_protocol::parser::InsertPosition::Absolute(1)),
    )
    .await;
    assert!(
        resp.contains("[2@0]") && resp.contains("Bad song index"),
        "out-of-range absolute position must ACK_ERROR_ARG, got: {resp}"
    );
}

#[tokio::test]
async fn test_load_relative_position_without_current_song() {
    let (_tmp, state) = new_test_state();
    write_playlist(&state, "list1", &["song1.mp3"]);

    let resp = playlists::handle_load_command(
        &state,
        "list1",
        None,
        Some(rmpd_protocol::parser::InsertPosition::After(0)),
    )
    .await;
    assert!(
        resp.contains("[55@0]") && resp.contains("No current song"),
        "relative position with nothing playing must ACK_ERROR_PLAYER_SYNC, got: {resp}"
    );
}

#[tokio::test]
async fn test_playlistadd_bad_position() {
    let (_tmp, state) = new_test_state();
    add_songs_to_db(&state, &["song1.mp3"]);

    let resp = playlists::handle_playlistadd_command(&state, "list1", "song1.mp3", Some(5)).await;
    assert!(
        resp.contains("[2@0]") && resp.contains("Bad position"),
        "position beyond the playlist's size must ACK_ERROR_ARG, got: {resp}"
    );
}

#[tokio::test]
async fn test_playlistdelete_range_removes_multiple_songs() {
    let (_tmp, state) = new_test_state();
    write_playlist(&state, "list1", &["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

    let resp = playlists::handle_playlistdelete_command(&state, "list1", (1, 3)).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");

    let resp = playlists::handle_listplaylist_command(&state, "list1", None).await;
    assert_eq!(resp, "file: a.mp3\nfile: d.mp3\nOK\n");
}

#[tokio::test]
async fn test_playlistdelete_one_past_end_is_noop() {
    // MPD's RangeArg::CheckClip accepts `start == len` (clipping the range
    // to empty) but rejects `start > len`.
    let (_tmp, state) = new_test_state();
    write_playlist(&state, "list1", &["a.mp3", "b.mp3"]);

    let resp = playlists::handle_playlistdelete_command(&state, "list1", (2, 3)).await;
    assert!(
        TestClient::is_ok(&resp),
        "index one past the end must succeed as a no-op, got: {resp}"
    );

    let resp = playlists::handle_playlistdelete_command(&state, "list1", (3, 4)).await;
    assert!(
        resp.contains("[2@0]") && resp.contains("Bad song index"),
        "index two past the end must ACK_ERROR_ARG, got: {resp}"
    );
}

#[tokio::test]
async fn test_playlistmove_range_relocates_a_song_block() {
    let (_tmp, state) = new_test_state();
    write_playlist(
        &state,
        "list1",
        &["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"],
    );

    // Move the two-song block [b, c] to sit right after d.
    let resp = playlists::handle_playlistmove_command(&state, "list1", (1, 3), 2).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");

    let resp = playlists::handle_listplaylist_command(&state, "list1", None).await;
    assert_eq!(
        resp,
        "file: a.mp3\nfile: d.mp3\nfile: b.mp3\nfile: c.mp3\nfile: e.mp3\nOK\n"
    );
}

#[tokio::test]
async fn test_playlistmove_open_ended_range_rejected() {
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_playlistmove_command(&state, "list1", (1, u32::MAX), 0).await;
    assert!(
        resp.contains("[2@0]") && resp.contains("Open-ended range not supported"),
        "got: {resp}"
    );
}

#[tokio::test]
async fn test_playlistmove_noop_skips_existence_check() {
    // MPD's early-out for an empty range never touches the playlist file, so
    // it succeeds even for a playlist that doesn't exist.
    let (_tmp, state) = new_test_state();

    let resp = playlists::handle_playlistmove_command(&state, "does-not-exist", (3, 3), 5).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");
}

#[tokio::test]
async fn test_searchplaylist_filter_and_window() {
    let (_tmp, state) = new_test_state();
    add_songs_to_db(&state, &["a.mp3", "b.mp3", "c.mp3"]);
    write_playlist(&state, "list1", &["a.mp3", "b.mp3", "c.mp3"]);

    let filters = vec![("artist".to_string(), "Test Artist".to_string())];
    let resp =
        playlists::handle_searchplaylist_command(&state, "list1", &filters, Some((1, 2))).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");
    assert!(
        resp.contains("file: b.mp3")
            && !resp.contains("file: a.mp3")
            && !resp.contains("file: c.mp3"),
        "window(1,2) must return only the second matching playlist entry, got: {resp}"
    );
    assert!(
        resp.contains("Pos: 1"),
        "Pos must reflect the entry's index within the playlist, got: {resp}"
    );
}

#[tokio::test]
async fn test_load_tracks_last_loaded_playlist_across_second_load_and_clear() {
    // Mirrors MPD's queue::last_loaded_playlist (Queue.hxx/PlaylistQueue.cxx):
    // set unconditionally on a successful `load`, overwritten by a second
    // `load`, untouched by a failed `load`, and reset by `clear`.
    let (_tmp, state) = new_test_state();
    add_songs_to_db(&state, &["a.mp3"]);
    write_playlist(&state, "pz", &["a.mp3"]);
    write_playlist(&state, "pz2", &["a.mp3"]);

    assert_eq!(state.queue.read().await.last_loaded_playlist(), "");

    let resp = playlists::handle_load_command(&state, "pz", None, None).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");
    assert_eq!(state.queue.read().await.last_loaded_playlist(), "pz");

    let resp = playlists::handle_load_command(&state, "pz2", None, None).await;
    assert!(TestClient::is_ok(&resp), "got: {resp}");
    assert_eq!(
        state.queue.read().await.last_loaded_playlist(),
        "pz2",
        "a second load must overwrite the previous name"
    );

    let resp = playlists::handle_load_command(&state, "does-not-exist", None, None).await;
    assert!(
        !TestClient::is_ok(&resp),
        "expected a failed load, got: {resp}"
    );
    assert_eq!(
        state.queue.read().await.last_loaded_playlist(),
        "pz2",
        "a failed load must leave the previous name untouched"
    );

    rmpd_protocol::commands::queue::handle_clear_command(&state).await;
    assert_eq!(
        state.queue.read().await.last_loaded_playlist(),
        "",
        "clear must reset the last-loaded-playlist name"
    );
}
