use rmpd_core::event::EventBus;
/// Integration tests for embedded FLAC `CUESHEET` exposure (see
/// `rmpd_library::embedded_cue`): a whole-album FLAC with an embedded cue
/// sheet is scanned as a container directory of range-restricted virtual
/// tracks, alongside the still-playable physical file.
///
/// Fixtures are generated at test time with `ffmpeg` (a short sine-wave
/// FLAC) and `metaflac` (embedding the cue sheet as a `CUESHEET` Vorbis
/// comment and/or the binary `CUESHEET` metadata block); tests skip cleanly
/// when either tool isn't installed.
use rmpd_library::database::Database;
use rmpd_library::scanner::Scanner;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

macro_rules! require_tools {
    () => {
        if !have("ffmpeg") || !have("metaflac") {
            eprintln!("ffmpeg/metaflac not available - skipping test");
            return;
        }
    };
}

/// Generate a `duration`-second sine-wave FLAC at `out` via ffmpeg, tagged
/// with basic TITLE/ARTIST/ALBUM (the container's own fallback tags).
fn generate_flac(out: &Path, duration_secs: u32) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:duration={duration_secs}"),
            "-ar",
            "44100",
            "-ac",
            "2",
            "-metadata",
            "title=Whole Album",
            "-metadata",
            "artist=Container Artist",
            "-metadata",
            "album=Test Album",
        ])
        .arg(out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg fixture generation failed");
}

const CUE_TEXT: &str = "TITLE \"Test Album\"\n\
PERFORMER \"Disc Artist\"\n\
FILE \"whatever-original.wav\" WAVE\n\
  TRACK 01 AUDIO\n\
    TITLE \"One\"\n\
    PERFORMER \"Artist One\"\n\
    INDEX 01 00:00:00\n\
  TRACK 02 AUDIO\n\
    TITLE \"Two\"\n\
    PERFORMER \"Artist Two\"\n\
    INDEX 01 00:03:00\n";

fn write_cue(dir: &Path) -> PathBuf {
    let cue_path = dir.join("album.cue");
    std::fs::write(&cue_path, CUE_TEXT).expect("write cue fixture");
    cue_path
}

/// Embed both the `CUESHEET` text comment and the binary metadata block
/// (`metaflac --import-cuesheet-from=... --set-tag-from-file=CUESHEET=...`,
/// matching the task's suggested invocation and real-world taggers).
fn embed_text_and_binary_cue(flac: &Path, cue: &Path) {
    let status = Command::new("metaflac")
        .arg(format!("--import-cuesheet-from={}", cue.display()))
        .arg(format!("--set-tag-from-file=CUESHEET={}", cue.display()))
        .arg(flac)
        .status()
        .expect("run metaflac");
    assert!(status.success(), "metaflac cuesheet import failed");
}

/// Embed only the binary `CUESHEET` metadata block (no text comment), to
/// exercise the indices-only fallback path.
fn embed_binary_cue_only(flac: &Path, cue: &Path) {
    let status = Command::new("metaflac")
        .arg(format!("--import-cuesheet-from={}", cue.display()))
        .arg(flac)
        .status()
        .expect("run metaflac");
    assert!(status.success(), "metaflac cuesheet import failed");
}

fn strip_cue(flac: &Path) {
    // metaflac rejects combining `--remove-tag` with `--remove
    // --block-type=` in one invocation ("you may not mix shorthand and
    // major operations"), so issue them as two separate calls.
    let status = Command::new("metaflac")
        .arg("--remove-tag=CUESHEET")
        .arg(flac)
        .status()
        .expect("run metaflac");
    assert!(status.success(), "metaflac cuesheet tag strip failed");
    let status = Command::new("metaflac")
        .args(["--remove", "--block-type=CUESHEET"])
        .arg(flac)
        .status()
        .expect("run metaflac");
    assert!(status.success(), "metaflac cuesheet block strip failed");
}

#[test]
fn embedded_text_cuesheet_becomes_virtual_tracks_with_cue_tags() {
    require_tools!();

    let temp = TempDir::new().expect("tempdir");
    let music_dir = temp.path().join("music");
    let artist_dir = music_dir.join("Artist");
    std::fs::create_dir_all(&artist_dir).expect("mkdir");

    let flac_path = artist_dir.join("Album.flac");
    generate_flac(&flac_path, 6);
    let cue_path = write_cue(temp.path());
    embed_text_and_binary_cue(&flac_path, &cue_path);

    let db_path = temp.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let scanner = Scanner::new(EventBus::new(), false);
    let stats = scanner.scan_directory(&database, &music_dir).expect("scan");
    assert_eq!(stats.errors, 0, "scan should not error");

    // The physical whole file is still a normal, independently playable song.
    let whole = database
        .get_song_by_path("Artist/Album.flac")
        .expect("query")
        .expect("whole file still present");
    assert!(whole.range.is_none(), "whole file has no range restriction");

    // `lsinfo Artist` shows both the physical file and the virtual container
    // directory named after it (MPD's `album.flac/trackNNNN` convention).
    let top = database.list_directory("Artist").expect("list Artist");
    assert!(
        top.songs.iter().any(|s| s.path == "Artist/Album.flac"),
        "physical file listed: {:?}",
        top.songs
            .iter()
            .map(|s| s.path.to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        top.directories
            .iter()
            .any(|(p, _)| p == "Artist/Album.flac"),
        "virtual container directory listed: {:?}",
        top.directories
    );

    // `lsinfo Artist/Album.flac` shows the two cue-derived tracks.
    let inside = database
        .list_directory("Artist/Album.flac")
        .expect("list container");
    assert_eq!(inside.songs.len(), 2, "two cue tracks");
    assert!(inside.directories.is_empty());

    let t1 = inside
        .songs
        .iter()
        .find(|s| s.path == "Artist/Album.flac/track0001")
        .expect("track0001 present");
    let t2 = inside
        .songs
        .iter()
        .find(|s| s.path == "Artist/Album.flac/track0002")
        .expect("track0002 present");

    assert_eq!(t1.tag("title"), Some("One"));
    assert_eq!(t1.tag("artist"), Some("Artist One"));
    assert_eq!(t1.range, Some((0.0, 3.0)));
    assert_eq!(
        t1.duration.map(|d| d.as_secs()),
        Some(3),
        "track0001 is a 3s slice"
    );

    assert_eq!(t2.tag("title"), Some("Two"));
    assert_eq!(t2.tag("artist"), Some("Artist Two"));
    assert_eq!(t2.range, Some((3.0, 6.0)));

    // Disc-level album/performer from the cue sheet override the container's
    // own ALBUM tag (still fell back to it had the cue not specified one).
    assert_eq!(t1.tag("album"), Some("Test Album"));
}

#[test]
fn binary_cuesheet_only_falls_back_to_synthesized_titles() {
    require_tools!();

    let temp = TempDir::new().expect("tempdir");
    let music_dir = temp.path().join("music");
    std::fs::create_dir_all(&music_dir).expect("mkdir");

    let flac_path = music_dir.join("NoText.flac");
    generate_flac(&flac_path, 6);
    let cue_path = write_cue(temp.path());
    embed_binary_cue_only(&flac_path, &cue_path);

    let db_path = temp.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let scanner = Scanner::new(EventBus::new(), false);
    scanner.scan_directory(&database, &music_dir).expect("scan");

    let inside = database
        .list_directory("NoText.flac")
        .expect("list container");
    assert_eq!(
        inside.songs.len(),
        2,
        "binary cuesheet still yields 2 tracks"
    );
    // No text comment: titles fall back to "<container title> - Track N",
    // not the cue's own (unavailable) TITLE.
    let titles: Vec<_> = inside
        .songs
        .iter()
        .map(|s| s.tag("title").unwrap_or("").to_string())
        .collect();
    assert!(titles.contains(&"Whole Album - Track 1".to_string()));
    assert!(titles.contains(&"Whole Album - Track 2".to_string()));
    // Container's own ARTIST tag is inherited (the binary block carries none).
    assert!(
        inside
            .songs
            .iter()
            .all(|s| s.tag("artist") == Some("Container Artist")),
    );
}

#[test]
fn rescanning_after_stripping_the_cue_removes_the_virtual_tracks() {
    require_tools!();

    let temp = TempDir::new().expect("tempdir");
    let music_dir = temp.path().join("music");
    std::fs::create_dir_all(&music_dir).expect("mkdir");

    let flac_path = music_dir.join("Album.flac");
    generate_flac(&flac_path, 6);
    let cue_path = write_cue(temp.path());
    embed_text_and_binary_cue(&flac_path, &cue_path);

    let db_path = temp.path().join("test.db");
    let database = Database::open(db_path.to_str().unwrap()).expect("open database");
    let scanner = Scanner::new(EventBus::new(), false);
    scanner
        .scan_directory(&database, &music_dir)
        .expect("first scan");
    assert_eq!(
        database
            .list_directory("Album.flac")
            .expect("list container")
            .songs
            .len(),
        2
    );

    strip_cue(&flac_path);

    // `update` (mtime-gated) would also catch this in practice since
    // metaflac changes the file's mtime, but the acceptance criterion is a
    // forced `rescan`, so exercise that explicitly.
    let rescanner = scanner.with_force_rescan(true);
    rescanner
        .scan_directory(&database, &music_dir)
        .expect("rescan");

    assert!(
        database
            .get_song_by_path("Album.flac")
            .expect("query")
            .is_some(),
        "whole file must still be present"
    );
    let listing = database.list_directory_recursive("");
    // The (now-empty) virtual container directory is pruned; no more
    // `Album.flac/trackNNNN` rows should exist anywhere in the tree.
    let has_track_rows = listing
        .expect("recursive list")
        .iter()
        .any(|s| s.path.as_str().starts_with("Album.flac/track"));
    assert!(
        !has_track_rows,
        "virtual tracks removed after cue is stripped"
    );
}
