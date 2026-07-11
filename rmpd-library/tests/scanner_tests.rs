/// Regression tests for `Scanner::collect_audio_files`'s directory-tree walk.
use rmpd_core::event::EventBus;
use rmpd_library::database::Database;
use rmpd_library::scanner::Scanner;
use tempfile::TempDir;

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
