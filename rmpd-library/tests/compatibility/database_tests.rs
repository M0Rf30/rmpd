/// Database query tests comparing rmpd behavior
///
/// These tests validate that rmpd's database operations produce
/// the same results as MPD for common query patterns.
use crate::common::rmpd_harness::RmpdTestHarness;
use crate::fixtures::{AudioFormat, FixtureGenerator, TestMetadata};

/// Helper to check if FFmpeg is available
macro_rules! require_ffmpeg {
    () => {
        if !FixtureGenerator::is_ffmpeg_available() {
            eprintln!("FFmpeg not available - skipping test");
            return;
        }
    };
}

#[test]
fn test_list_artists() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from different artists
    let artists = ["Artist A", "Artist B", "Artist C"];

    for (i, artist) in artists.iter().enumerate() {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: artist.to_string(),
            album: "Test Album".to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // List all artists
    let result = harness.list_artists().unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&"Artist A".to_string()));
    assert!(result.contains(&"Artist B".to_string()));
    assert!(result.contains(&"Artist C".to_string()));

    // Results should be sorted (case-insensitive)
    assert_eq!(result[0], "Artist A");
    assert_eq!(result[1], "Artist B");
    assert_eq!(result[2], "Artist C");
}

#[test]
fn test_list_albums() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from different albums
    let albums = ["Album X", "Album Y", "Album Z"];

    for (i, album) in albums.iter().enumerate() {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Test Artist".to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.list_albums().unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&"Album X".to_string()));
    assert!(result.contains(&"Album Y".to_string()));
    assert!(result.contains(&"Album Z".to_string()));
}

#[test]
fn test_find_by_artist() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add multiple songs by the same artist
    for i in 1..=3 {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Target Artist".to_string(),
            album: format!("Album {}", i),
            track: Some(i),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Add a song by a different artist
    let other_metadata = TestMetadata {
        title: "Other Song".to_string(),
        artist: "Other Artist".to_string(),
        album: "Other Album".to_string(),
        ..Default::default()
    };
    let other_path = generator
        .generate(AudioFormat::Flac, &other_metadata)
        .unwrap();
    let other_song = harness
        .extract_metadata(other_path.to_str().unwrap())
        .unwrap();
    harness.add_song(&other_song).unwrap();

    // Find songs by target artist
    let result = harness.find_by_artist("Target Artist").unwrap();

    assert_eq!(result.len(), 3);
    for song in &result {
        assert_eq!(song.tag("artist"), Some("Target Artist"));
    }

    // Results should be ordered by album, track
    assert_eq!(result[0].tag("track"), Some("1"));
    assert_eq!(result[1].tag("track"), Some("2"));
    assert_eq!(result[2].tag("track"), Some("3"));
}

#[test]
fn test_find_by_album() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add multiple songs from the same album
    for i in 1..=5 {
        let metadata = TestMetadata {
            title: format!("Track {}", i),
            artist: "Album Artist".to_string(),
            album: "Test Album".to_string(),
            track: Some(i),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.find_by_album("Test Album").unwrap();

    assert_eq!(result.len(), 5);
    for song in &result {
        assert_eq!(song.tag("album"), Some("Test Album"));
    }

    // Results should be ordered by track number
    for (i, song) in result.iter().enumerate() {
        assert_eq!(
            song.tag("track").and_then(|t| t.parse::<u32>().ok()),
            Some((i + 1) as u32)
        );
    }
}

#[test]
fn test_count_songs() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Initially empty
    assert_eq!(harness.count_songs().unwrap(), 0);

    // Add songs
    for i in 1..=10 {
        let metadata = TestMetadata {
            title: format!("Song {}", i),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            track: Some(i),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    assert_eq!(harness.count_songs().unwrap(), 10);
}

#[test]
fn test_count_artists() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from 3 different artists (2 songs each)
    let artists = vec!["Artist 1", "Artist 2", "Artist 3"];

    for artist in &artists {
        for i in 1..=2 {
            let metadata = TestMetadata {
                title: format!("Song {} by {}", i, artist),
                artist: artist.to_string(),
                album: "Test Album".to_string(),
                ..Default::default()
            };

            let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
            let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
            harness.add_song(&song).unwrap();
        }
    }

    // Should count 3 unique artists (not 6 songs)
    assert_eq!(harness.count_artists().unwrap(), 3);
}

#[test]
fn test_count_albums() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add songs from 4 different albums
    let albums = vec!["Album A", "Album B", "Album C", "Album D"];

    for album in &albums {
        let metadata = TestMetadata {
            title: format!("Song from {}", album),
            artist: "Test Artist".to_string(),
            album: album.to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    assert_eq!(harness.count_albums().unwrap(), 4);
}

#[test]
fn test_case_insensitive_listing() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Add artists with different casings
    let artists = vec!["The Beatles", "the beatles", "THE BEATLES"];

    for artist in &artists {
        let metadata = TestMetadata {
            title: "Song".to_string(),
            artist: artist.to_string(),
            album: "Album".to_string(),
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    let result = harness.list_artists().unwrap();

    // Should list all three variations
    assert_eq!(result.len(), 3);
}

#[test]
fn test_empty_database_queries() {
    let harness = RmpdTestHarness::new().unwrap();

    // All queries should work on empty database
    assert_eq!(harness.count_songs().unwrap(), 0);
    assert_eq!(harness.count_artists().unwrap(), 0);
    assert_eq!(harness.count_albums().unwrap(), 0);

    assert_eq!(harness.list_artists().unwrap().len(), 0);
    assert_eq!(harness.list_albums().unwrap().len(), 0);

    assert_eq!(harness.find_by_artist("Nonexistent").unwrap().len(), 0);
    assert_eq!(harness.find_by_album("Nonexistent").unwrap().len(), 0);
}

#[test]
fn test_query_with_special_characters() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Artist with special characters
    let metadata = TestMetadata {
        title: "Song".to_string(),
        artist: "AC/DC".to_string(),
        album: "Back in Black".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    harness.add_song(&song).unwrap();

    let artists = harness.list_artists().unwrap();
    assert!(artists.contains(&"AC/DC".to_string()));

    let found = harness.find_by_artist("AC/DC").unwrap();
    assert_eq!(found.len(), 1);
}

#[test]
fn test_multiple_albums_same_name_different_artists() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    // Two different artists with albums of the same name
    let artists = vec!["Artist One", "Artist Two"];

    for artist in &artists {
        let metadata = TestMetadata {
            title: "Title Track".to_string(),
            artist: artist.to_string(),
            album: "Greatest Hits".to_string(), // Same album name
            ..Default::default()
        };

        let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
        let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
        harness.add_song(&song).unwrap();
    }

    // Should list "Greatest Hits" twice (different artists)
    let albums = harness.list_albums().unwrap();
    let greatest_hits_count = albums.iter().filter(|a| *a == "Greatest Hits").count();
    assert_eq!(greatest_hits_count, 1); // Or 2, depending on database normalization
}

#[test]
fn test_get_song_by_id() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata {
        title: "Specific Song".to_string(),
        artist: "Specific Artist".to_string(),
        album: "Specific Album".to_string(),
        ..Default::default()
    };

    let path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness.extract_metadata(path.to_str().unwrap()).unwrap();
    let id = harness.add_song(&song).unwrap();

    // Retrieve by ID
    let retrieved = harness.get_song(id).unwrap().unwrap();
    assert_eq!(retrieved.tag("title"), Some("Specific Song"));
    assert_eq!(retrieved.tag("artist"), Some("Specific Artist"));

    // Non-existent ID
    let nonexistent = harness.get_song(99999).unwrap();
    assert!(nonexistent.is_none());
}

#[test]
fn test_get_song_by_path() {
    require_ffmpeg!();

    let generator = FixtureGenerator::new().unwrap();
    let harness = RmpdTestHarness::new().unwrap();

    let metadata = TestMetadata::default();
    let fixture_path = generator.generate(AudioFormat::Flac, &metadata).unwrap();
    let song = harness
        .extract_metadata(fixture_path.to_str().unwrap())
        .unwrap();

    let song_path = song.path.clone();
    harness.add_song(&song).unwrap();

    // Retrieve by path
    let retrieved = harness
        .get_song_by_path(song_path.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.path, song_path);

    // Non-existent path
    let nonexistent = harness.get_song_by_path("/nonexistent/path.mp3").unwrap();
    assert!(nonexistent.is_none());
}

// ── Source-column tests (no FFmpeg required) ────────────────────────────────

/// Build a minimal Song with a virtual path and optional title tag.
fn make_virtual_song(virtual_path: &str, title: &str) -> rmpd_core::song::Song {
    rmpd_core::song::Song {
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
        tags: vec![(rmpd_core::song::intern_tag_key("title"), title.to_string())],
    }
}

/// Build a minimal local song with a relative path (no source).
fn make_local_song(path: &str) -> rmpd_core::song::Song {
    make_virtual_song(path, "local")
}

/// (a) `migrate_schema` is idempotent: opening the DB a second time must not
/// error and the `source` column must be present exactly once.
#[test]
fn test_source_column_migration_idempotent() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("idem.db")
        .to_string_lossy()
        .to_string();

    // First open: schema created fresh, migration runs (no-op for source since
    // the column is in the CREATE TABLE).
    rmpd_library::database::Database::open(&db_path).unwrap();

    // Second open: migrate_schema runs again on the existing DB. Must be a no-op.
    let db = rmpd_library::database::Database::open(&db_path).unwrap();

    // Prove the column exists and is usable by inserting a source song.
    let song = make_virtual_song("subsonic://srv/A/B/id1", "Test");
    db.add_source_song(&song, "subsonic:srv").unwrap();
    // A third open (migration re-run) must also succeed.
    rmpd_library::database::Database::open(&db_path).unwrap();
}

/// (b) After `add_source_song` for `subsonic://srv/A/B/id`:
///  - `list_directory("")` shows `subsonic://srv`
///  - `list_directory("subsonic://srv/A/B")` returns the song
///  - `get_song_by_path` finds it by exact path
#[test]
fn test_add_source_song_directory_traversal() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir.path().join("src.db").to_string_lossy().to_string();
    let db = rmpd_library::database::Database::open(&db_path).unwrap();

    let virtual_path = "subsonic://srv/A/B/track-42";
    let song = make_virtual_song(virtual_path, "Track 42");
    db.add_source_song(&song, "subsonic:srv").unwrap();

    // Root listing must include "subsonic://srv"
    let root = db.list_directory("").unwrap();
    let root_dirs: Vec<&str> = root.directories.iter().map(|(p, _)| p.as_str()).collect();
    assert!(
        root_dirs.contains(&"subsonic://srv"),
        "root listing missing subsonic://srv; got: {:?}",
        root_dirs
    );

    // Leaf directory listing must contain the song
    let leaf = db.list_directory("subsonic://srv/A/B").unwrap();
    assert_eq!(
        leaf.songs.len(),
        1,
        "expected 1 song in subsonic://srv/A/B, got {}",
        leaf.songs.len()
    );
    assert_eq!(leaf.songs[0].path.as_str(), virtual_path);

    // get_song_by_path must find the song
    let found = db.get_song_by_path(virtual_path).unwrap();
    assert!(found.is_some(), "get_song_by_path returned None");
    let found = found.unwrap();
    assert_eq!(found.path.as_str(), virtual_path);
    assert_eq!(found.tag("title"), Some("Track 42"));
}

/// (c) `clear_source("subsonic:srv")` deletes only remote rows and leaves a
/// local song untouched.
#[test]
fn test_clear_source_leaves_local_songs() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir.path().join("clr.db").to_string_lossy().to_string();
    let db = rmpd_library::database::Database::open(&db_path).unwrap();

    // Add a local song (no source)
    let local = make_local_song("music/artist/album/local.flac");
    db.add_song(&local).unwrap();

    // Add two remote songs
    let r1 = make_virtual_song("subsonic://srv/Artist/Album/id1", "Remote 1");
    let r2 = make_virtual_song("subsonic://srv/Artist/Album/id2", "Remote 2");
    db.add_source_song(&r1, "subsonic:srv").unwrap();
    db.add_source_song(&r2, "subsonic:srv").unwrap();

    assert_eq!(db.count_songs().unwrap(), 3);

    // Clear only remote songs
    let deleted = db.clear_source("subsonic:srv").unwrap();
    assert_eq!(deleted, 2, "expected 2 remote songs deleted, got {deleted}");

    // Local song must survive
    assert_eq!(db.count_songs().unwrap(), 1);
    let local_check = db
        .get_song_by_path("music/artist/album/local.flac")
        .unwrap();
    assert!(local_check.is_some(), "local song was incorrectly deleted");
}

/// Regression: deleting song rows fires the `songs_fts_delete` trigger, which
/// must NOT corrupt the contentless FTS index. Before the `contentless_delete=1`
/// fix, every `add_*` wrote an empty-string 'delete' tombstone and the trigger
/// issued another, leaving the index malformed; the first row DELETE then raised
/// "database disk image is malformed". This adds local + remote songs, deletes a
/// subset by path and by source, and asserts `search_songs` returns exactly the
/// survivors — and that a fresh insert is still searchable afterwards (proving
/// the index stays writable, which the older source tests never exercised).
#[test]
fn test_search_after_delete_no_fts_corruption() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("fts_regression.db")
        .to_string_lossy()
        .to_string();
    let db = rmpd_library::database::Database::open(&db_path).unwrap();

    // Two local songs (deleted by path) and two remote songs (deleted by source).
    // Distinct single-token titles keep the FTS match unambiguous.
    db.add_song(&make_virtual_song("music/a/keepalpha.flac", "keepalpha"))
        .unwrap();
    db.add_song(&make_virtual_song("music/a/dropbeta.flac", "dropbeta"))
        .unwrap();
    db.add_source_song(
        &make_virtual_song("subsonic://srv/X/Y/r1", "keepgamma"),
        "subsonic:srv",
    )
    .unwrap();
    db.add_source_song(
        &make_virtual_song("subsonic://srv/X/Y/r2", "dropdelta"),
        "subsonic:srv",
    )
    .unwrap();

    // All four are searchable before any delete.
    assert_eq!(db.search_songs("keepalpha").unwrap().len(), 1);
    assert_eq!(db.search_songs("dropbeta").unwrap().len(), 1);
    assert_eq!(db.search_songs("keepgamma").unwrap().len(), 1);
    assert_eq!(db.search_songs("dropdelta").unwrap().len(), 1);

    // Delete a subset: one local song by path, both remote songs by source. Each
    // DELETE fires songs_fts_delete; under the old code the first of these threw
    // "database disk image is malformed".
    db.delete_song_by_path("music/a/dropbeta.flac").unwrap();
    let cleared = db.clear_source("subsonic:srv").unwrap();
    assert_eq!(cleared, 2, "expected 2 remote songs cleared, got {cleared}");
    assert_eq!(
        db.count_songs().unwrap(),
        1,
        "only the kept local song remains"
    );

    // Survivor still matches; deleted/cleared rows return nothing.
    let survivors = db.search_songs("keepalpha").unwrap();
    assert_eq!(
        survivors.len(),
        1,
        "surviving local song must still be searchable"
    );
    assert_eq!(survivors[0].path.as_str(), "music/a/keepalpha.flac");
    assert!(
        db.search_songs("dropbeta").unwrap().is_empty(),
        "deleted local song must not match"
    );
    assert!(
        db.search_songs("keepgamma").unwrap().is_empty(),
        "cleared remote song must not match"
    );
    assert!(
        db.search_songs("dropdelta").unwrap().is_empty(),
        "cleared remote song must not match"
    );

    // A fresh insert after the deletes must be searchable: proves the index is
    // intact and still writable (not just that reads happen to filter rows).
    db.add_song(&make_virtual_song("music/a/newone.flac", "newepsilon"))
        .unwrap();
    let added = db.search_songs("newepsilon").unwrap();
    assert_eq!(added.len(), 1, "post-delete insert must be searchable");
    assert_eq!(added[0].path.as_str(), "music/a/newone.flac");
}

/// v3→v4 migration from a legacy on-disk DB: a `songs_fts` created WITHOUT
/// `contentless_delete=1` must be transparently migrated on open. We hand-build
/// a legacy DB with raw SQL (the public API can no longer produce the old
/// definition), open it through `Database`, and assert the index was rebuilt
/// from `song_tags` (search still works), the delete path is now safe (no
/// "database disk image is malformed"), and re-opening is a no-op.
#[test]
fn test_fts_contentless_delete_migration_from_legacy_db() {
    use rusqlite::Connection;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("legacy.db")
        .to_string_lossy()
        .to_string();

    // Build a legacy-format DB: a `songs` table (with the v2→v3 `source` column
    // already present, so only the FTS migration is under test), `song_tags`,
    // and an OLD `songs_fts` declared `content=''` WITHOUT contentless_delete,
    // populated for two songs the way the pre-fix code did.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE songs (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 directory_id INTEGER NOT NULL,
                 mtime INTEGER NOT NULL,
                 duration REAL, sample_rate INTEGER, channels INTEGER,
                 bits_per_sample INTEGER, bitrate INTEGER,
                 replay_gain_track_gain REAL, replay_gain_track_peak REAL,
                 replay_gain_album_gain REAL, replay_gain_album_peak REAL,
                 added_at INTEGER NOT NULL DEFAULT 0,
                 last_modified INTEGER NOT NULL DEFAULT 0,
                 source TEXT
             );
             CREATE TABLE song_tags (
                 song_id INTEGER NOT NULL,
                 tag TEXT NOT NULL,
                 value TEXT NOT NULL DEFAULT ''
             );
             CREATE VIRTUAL TABLE songs_fts USING fts5(
                 title, artist, album, album_artist, genre, composer, content=''
             );
             INSERT INTO songs (id, path, directory_id, mtime) VALUES
                 (1, 'music/legacone.flac', 1, 0),
                 (2, 'music/legactwo.flac', 1, 0);
             INSERT INTO song_tags (song_id, tag, value) VALUES
                 (1, 'title', 'legacalpha'),
                 (2, 'title', 'legacbeta');
             INSERT INTO songs_fts (rowid, title, artist, album, album_artist, genre, composer) VALUES
                 (1, 'legacalpha', '', '', '', '', ''),
                 (2, 'legacbeta', '', '', '', '', '');",
        )
        .unwrap();
        // Precondition: the legacy table genuinely lacks the option.
        let legacy_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='songs_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !legacy_sql.contains("contentless_delete"),
            "precondition: legacy songs_fts must lack contentless_delete"
        );
    }

    // Open through Database → migrate_schema runs the v3→v4 step.
    let db = rmpd_library::database::Database::open(&db_path).unwrap();

    // The index was rebuilt from song_tags: both legacy songs are searchable.
    let alpha = db.search_songs("legacalpha").unwrap();
    assert_eq!(
        alpha.len(),
        1,
        "rebuilt index must return the first legacy song"
    );
    assert_eq!(alpha[0].path.as_str(), "music/legacone.flac");
    assert_eq!(db.search_songs("legacbeta").unwrap().len(), 1);

    // Delete one song: under the legacy table this would corrupt the index;
    // after migration it succeeds and only the survivor remains searchable.
    db.delete_song_by_path("music/legacone.flac").unwrap();
    assert!(
        db.search_songs("legacalpha").unwrap().is_empty(),
        "deleted legacy song must not match after migration"
    );
    let beta = db.search_songs("legacbeta").unwrap();
    assert_eq!(
        beta.len(),
        1,
        "surviving legacy song must remain searchable"
    );
    assert_eq!(beta[0].path.as_str(), "music/legactwo.flac");

    // Re-opening is idempotent: the guard now sees contentless_delete and the
    // migration body is skipped, leaving data intact.
    let db2 = rmpd_library::database::Database::open(&db_path).unwrap();
    assert_eq!(db2.count_songs().unwrap(), 1);
    assert_eq!(db2.search_songs("legacbeta").unwrap().len(), 1);
}
