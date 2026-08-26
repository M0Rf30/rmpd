/// Regression tests for `Scanner::collect_audio_files`'s directory-tree walk.
use rmpd_core::event::{Event, EventBus};
use rmpd_core::song::Song;
use rmpd_library::database::Database;
use rmpd_library::scanner::Scanner;
use rmpd_library::watcher::FilesystemWatcher;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Mutex;

/// Build a minimal remote/virtual song for `Database::add_source_song`, whose
/// mount-style path convention is `<mount>/<segments>.../<leaf>` (see that
/// method's doc comment).
fn remote_song(virtual_path: &str) -> Song {
    Song {
        id: 0,
        path: virtual_path.into(),
        duration: None,
        sample_rate: None,
        channels: None,
        bits_per_sample: None,
        bitrate: None,
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        tags: Vec::new(),
    }
}

/// A symlink that points back at an ancestor directory (or otherwise forms a
/// cycle) must not send the scanner into deep/unbounded recursion when
/// `follow_symlinks` is enabled.
///
/// Without the (dev, ino) cycle guard, the walk keeps re-entering the same
/// directory through the symlink, growing the traversed path on every
/// recursive call (`music/sub/loop/sub/loop/...`) until the kernel's own
/// symlink-resolution limit (`ELOOP`) finally rejects the path and the scan
/// records that as an error. With the guard, the second time the walk would
/// re-enter the already-visited directory it is skipped immediately instead,
/// so the scan finishes with no errors at all. Asserting `errors == 0` (not
/// just `is_ok()`, since `collect_audio_files` already swallows per-directory
/// errors) is what actually distinguishes "cycle detected up front" from
/// "recursed dozens of levels deep before the OS bailed us out" and would
/// fail if the cycle guard regressed.
#[test]
fn scan_with_symlink_cycle_terminates() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let music_dir = temp_dir.path().join("music");
    std::fs::create_dir(&music_dir).expect("create music dir");

    // music/sub is a real subdirectory...
    let sub_dir = music_dir.join("sub");
    std::fs::create_dir(&sub_dir).expect("create sub dir");

    // ...that contains a symlink pointing back at its own ancestor (the music
    // root), forming a cycle: music/sub/loop -> music -> sub -> loop -> ...
    let loop_link = sub_dir.join("loop");
    std::os::unix::fs::symlink(&music_dir, &loop_link).expect("create symlink cycle");

    let db_path = temp_dir.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");

    // follow_symlinks: true is required to reproduce the cycle at all.
    let scanner = Scanner::new(EventBus::new(), true);

    let result = scanner.scan_directory(&database, &music_dir);

    let stats = result.expect("scan should complete despite the symlink cycle");
    assert_eq!(
        stats.errors, 0,
        "cycle guard should skip the already-visited directory on first re-entry \
         instead of recursing until the OS's own symlink-loop limit errors out"
    );
}

/// `handle_fs_event` (the watcher's create/modify handler) must store songs
/// under the same music-dir-relative path the scanner uses. It used to insert
/// the absolute path returned by `MetadataExtractor::extract_from_file`
/// directly, so `get_song_by_path(relative)` — used by lsinfo/add/
/// playlistinfo/stickers — could never find a watcher-added file.
#[tokio::test]
async fn watcher_stores_relative_path_for_new_file() {
    let temp_dir = TempDir::new().expect("create temp dir");
    std::fs::create_dir(temp_dir.path().join("music")).expect("create music dir");
    // Resolve symlinks (macOS puts temp dirs behind /var -> /private/var), so the
    // watcher's music-dir prefix matches the paths the OS reports for events.
    let music_dir = std::fs::canonicalize(temp_dir.path().join("music")).expect("canonicalize");

    let db_path = temp_dir.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let db = Arc::new(Mutex::new(database));

    let mut watcher = FilesystemWatcher::new(music_dir.clone(), db.clone(), EventBus::new())
        .expect("create watcher");
    watcher.start().await.expect("start watcher");

    // Let the watcher's spawned event-handler task hook up before the
    // debounced filesystem event arrives.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples/basic.flac");
    std::fs::copy(&fixture, music_dir.join("song.flac")).expect("copy fixture into music dir");

    // Debounce is 300ms, but filesystem-event latency varies by backend
    // (inotify vs FSEvents), so poll instead of sleeping a fixed amount.
    let mut song = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        song = db
            .lock()
            .await
            .get_song_by_path("song.flac")
            .expect("query by relative path");
        if song.is_some() {
            break;
        }
    }

    let song = song.expect(
        "watcher should store the new file under its music-dir-relative \
         path, not the absolute path returned by extract_from_file",
    );
    assert_eq!(song.path.as_str(), "song.flac");
}

/// A song row whose file has been deleted from disk between two `update`s
/// must be pruned from the database (issue #12): songs that vanish from disk
/// were never removed, so `findadd` + `play` would "succeed" and then fail to
/// open the file. `scan_directory` must delete the row and report it via
/// `ScanStats::removed`, leaving files that are still present untouched.
#[test]
fn update_prunes_songs_whose_files_are_gone() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let music_dir = temp_dir.path().join("music");
    std::fs::create_dir(&music_dir).expect("create music dir");

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples/basic.flac");
    std::fs::copy(&fixture, music_dir.join("a.flac")).expect("copy fixture as a.flac");
    std::fs::copy(&fixture, music_dir.join("b.flac")).expect("copy fixture as b.flac");

    let db_path = temp_dir.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let scanner = Scanner::new(EventBus::new(), false);

    let stats = scanner
        .scan_directory(&database, &music_dir)
        .expect("first scan");
    assert_eq!(
        stats.added, 2,
        "both files should be added on the first scan"
    );
    assert!(
        database.get_song_by_path("a.flac").unwrap().is_some(),
        "a.flac should be in the database after the first scan"
    );
    assert!(
        database.get_song_by_path("b.flac").unwrap().is_some(),
        "b.flac should be in the database after the first scan"
    );

    std::fs::remove_file(music_dir.join("b.flac")).expect("remove b.flac");

    let stats = scanner
        .scan_directory(&database, &music_dir)
        .expect("second scan");

    assert_eq!(stats.removed, 1, "b.flac's row should be pruned");
    assert_eq!(stats.added, 0, "no new files were added on the second scan");
    assert!(
        database.get_song_by_path("b.flac").unwrap().is_none(),
        "b.flac's row should be gone after the prune"
    );
    assert!(
        database.get_song_by_path("a.flac").unwrap().is_some(),
        "a.flac should be untouched since its file is still present"
    );
}

/// Remote catalog rows (`Database::add_source_song`) must never be evicted by
/// the local-filesystem prune, since `list_local_song_paths`/
/// `delete_song_by_path` are guarded with `source IS NULL`.
#[test]
fn update_keeps_remote_source_rows() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let music_dir = temp_dir.path().join("music");
    std::fs::create_dir(&music_dir).expect("create music dir");

    let db_path = temp_dir.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");

    let song = remote_song("mymount/Artist/Album/song.flac");
    database
        .add_source_song(&song, "subsonic")
        .expect("add remote song");

    let scanner = Scanner::new(EventBus::new(), false);
    let stats = scanner
        .scan_directory(&database, &music_dir)
        .expect("scan of an empty music dir");

    assert_eq!(
        stats.removed, 0,
        "remote rows must not be counted as pruned"
    );
    assert!(
        database
            .get_song_by_path("mymount/Artist/Album/song.flac")
            .unwrap()
            .is_some(),
        "the remote source row should survive a local scan with no matching file on disk"
    );
}

/// Pruning a missing song must emit `Event::SongDeleted` so `idle` clients
/// pick up the removal the same way they do for the watcher's own deletes.
#[test]
fn update_emits_song_deleted_for_pruned_rows() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let music_dir = temp_dir.path().join("music");
    std::fs::create_dir(&music_dir).expect("create music dir");

    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/samples/basic.flac");
    std::fs::copy(&fixture, music_dir.join("b.flac")).expect("copy fixture as b.flac");

    let db_path = temp_dir.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let event_bus = EventBus::new();
    let scanner = Scanner::new(event_bus.clone(), false);

    scanner
        .scan_directory(&database, &music_dir)
        .expect("first scan");

    // Subscribe only before the second scan: a new subscriber does not see
    // events emitted before it subscribed.
    let mut rx = event_bus.subscribe();

    std::fs::remove_file(music_dir.join("b.flac")).expect("remove b.flac");
    scanner
        .scan_directory(&database, &music_dir)
        .expect("second scan");

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let deleted = events
        .iter()
        .position(|e| matches!(e, Event::SongDeleted { path } if path == "b.flac"))
        .expect("pruning b.flac's row should emit Event::SongDeleted");
    let finished = events
        .iter()
        .position(|e| matches!(e, Event::DatabaseUpdateFinished))
        .expect("the scan should emit Event::DatabaseUpdateFinished");
    assert!(
        deleted < finished,
        "SongDeleted must be emitted before DatabaseUpdateFinished, so a client \
         refreshing on the update event already sees the pruned database"
    );
}
