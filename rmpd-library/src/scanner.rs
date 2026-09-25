use camino::Utf8PathBuf;
use rayon::prelude::*;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event, EventBus};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::metadata::MetadataExtractor;
use rmpd_core::time::system_time_to_unix_secs;

/// Information about a file to be processed
#[derive(Debug, Clone)]
struct FileInfo {
    absolute_path: Utf8PathBuf,
    relative_path: Utf8PathBuf,
    existing_song: Option<rmpd_core::song::Song>,
    file_size: u64,
}

/// Result of metadata extraction for a file
#[derive(Debug)]
struct ExtractedMetadata {
    file_info: FileInfo,
    song: Option<rmpd_core::song::Song>,
    /// Embedded-cue virtual tracks derived from `song` (see
    /// `crate::embedded_cue`), empty when `song` has no embedded cue sheet
    /// (or isn't a FLAC). Always synced (even when empty, to drop a cue
    /// sheet that was removed) alongside `song` in the batch-insert step.
    container_tracks: Vec<rmpd_core::song::Song>,
    error: Option<String>,
}

#[derive(Debug)]
pub struct Scanner {
    event_bus: EventBus,
    music_directory: Option<Utf8PathBuf>,
    /// Follow a symlink whose target resolves inside `music_directory`.
    /// Matches mpd.conf's `follow_inside_symlinks` (mpd
    /// `src/db/update/Config.cxx`, default yes).
    follow_inside_symlinks: bool,
    /// Follow a symlink whose target resolves outside `music_directory`.
    /// Matches mpd.conf's `follow_outside_symlinks` (default yes).
    follow_outside_symlinks: bool,
    force_rescan: bool,
}

/// A single `.mpdignore` glob pattern, matched against a file/directory's
/// NAME only (never the full path) — mpd `src/db/update/ExcludeList.cxx`
/// checks the bare entry name, not a path relative to the scan root.
type IgnorePattern = String;

/// Shell-style glob match supporting `*` (any run of characters) and `?`
/// (any single character), matching mpd's `Glob::Check`, which is backed by
/// `fnmatch(pattern, name, 0)` (mpd `src/fs/Glob.hxx`). No character
/// classes or brace expansion: MPD's own patterns don't use them either.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_match = 0usize;

    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_match = ni;
            pi += 1;
        } else if let Some(si) = star {
            pi = si + 1;
            star_match += 1;
            ni = star_match;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Parses a `.mpdignore` file's contents into glob patterns. Blank lines and
/// lines starting with `#` are skipped; every other line is stripped and
/// used verbatim, mirroring `ExcludeList::ParseLine`
/// (mpd `src/db/update/ExcludeList.cxx`).
pub(crate) fn parse_mpdignore(content: &str) -> Vec<IgnorePattern> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Reads and parses `dir`'s own `.mpdignore`, if any. A missing file is not
/// an error (most directories don't have one); any other read error is
/// logged and treated as "no patterns", matching mpd's
/// `LoadExcludeListOrLog`.
pub(crate) fn load_mpdignore(dir: &Path) -> Vec<IgnorePattern> {
    match fs::read_to_string(dir.join(".mpdignore")) {
        Ok(content) => parse_mpdignore(&content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            warn!("failed to read {:?}/.mpdignore: {}", dir, e);
            Vec::new()
        }
    }
}

/// Whether `name` matches any pattern in `patterns`. `patterns` is the
/// flattened union of a directory's own `.mpdignore` and every ancestor's,
/// which is equivalent to mpd's parent-chained `ExcludeList::Check`
/// (mpd `src/db/update/ExcludeList.cxx`): a match at any level excludes.
pub(crate) fn is_mpdignore_excluded(patterns: &[IgnorePattern], name: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, name))
}

impl Scanner {
    /// `follow_symlinks` sets both the inside- and outside-`music_directory`
    /// policies to the same value. Use `with_symlink_policy` to set them
    /// independently (mpd's `follow_inside_symlinks`/`follow_outside_symlinks`).
    pub fn new(event_bus: EventBus, follow_symlinks: bool) -> Self {
        Self {
            event_bus,
            music_directory: None,
            follow_inside_symlinks: follow_symlinks,
            follow_outside_symlinks: follow_symlinks,
            force_rescan: false,
        }
    }

    /// Configure the two independent MPD-style symlink-follow flags (mpd
    /// `src/db/update/Config.cxx`: both default to yes).
    pub fn with_symlink_policy(
        &self,
        follow_inside_symlinks: bool,
        follow_outside_symlinks: bool,
    ) -> Self {
        Self {
            event_bus: self.event_bus.clone(),
            music_directory: self.music_directory.clone(),
            follow_inside_symlinks,
            follow_outside_symlinks,
            force_rescan: self.force_rescan,
        }
    }

    /// Returns a copy of this scanner with `music_directory` set to `dir`.
    ///
    /// `scan_directory` uses this instead of inline struct construction so that if
    /// `Scanner` gains new fields in the future only this one place needs updating.
    pub fn with_music_dir(&self, dir: Utf8PathBuf) -> Self {
        Self {
            event_bus: self.event_bus.clone(),
            music_directory: Some(dir),
            follow_inside_symlinks: self.follow_inside_symlinks,
            follow_outside_symlinks: self.follow_outside_symlinks,
            force_rescan: self.force_rescan,
        }
    }

    /// When `true`, re-reads tags for every file even if its on-disk mtime
    /// hasn't advanced past the database's recorded `last_modified` —
    /// matches MPD's `rescan` command (`update` only re-reads modified
    /// files, `rescan` also rescans unmodified ones).
    pub fn with_force_rescan(&self, force: bool) -> Self {
        Self {
            event_bus: self.event_bus.clone(),
            music_directory: self.music_directory.clone(),
            follow_inside_symlinks: self.follow_inside_symlinks,
            follow_outside_symlinks: self.follow_outside_symlinks,
            force_rescan: force,
        }
    }

    pub fn scan_directory(&self, db: &Database, root_path: &Path) -> Result<ScanStats> {
        info!("starting music library scan: {}", root_path.display());
        self.event_bus.emit(Event::DatabaseUpdateStarted);

        let mut stats = ScanStats::default();

        // Build a scanner variant that knows the music directory so that make_relative_path
        // can strip the root prefix from absolute paths during the scan. If `self` already
        // has one configured (a scan of one of its own subtrees), keep it — `root_path` is
        // then a subtree root, not the library root, and prune_missing/file_is_present need
        // the real root to resolve database-relative paths back to disk.
        let utf8_root = Utf8PathBuf::try_from(root_path.to_path_buf())
            .map_err(|_| RmpdError::Library("Music directory path is not valid UTF-8".into()))?;
        let scanner_with_dir = self.with_music_dir(
            self.music_directory
                .clone()
                .unwrap_or_else(|| utf8_root.clone()),
        );

        scanner_with_dir.scan_recursive(db, root_path, &mut stats)?;

        let prefix = scanner_with_dir.make_relative_path(&utf8_root)?;
        scanner_with_dir.prune_missing(db, prefix.as_str(), &mut stats);
        scanner_with_dir.prune_empty_directories(db, &mut stats);

        info!(
            "scan complete: {} files scanned, {} added, {} updated, {} removed, {} errors",
            stats.scanned, stats.added, stats.updated, stats.removed, stats.errors
        );

        self.event_bus.emit(Event::DatabaseUpdateFinished);

        Ok(stats)
    }

    /// Delete local song rows at or under `prefix` whose file is no longer present on disk.
    ///
    /// `prefix` is the scanned subtree's database-relative path (`""` for a whole-library
    /// scan, which is what every caller passes today), computed by `scan_directory` from the
    /// scanner's `music_directory` and the scan root — so a scan rooted at a subdirectory only
    /// ever considers rows under that subdirectory, never every row outside it.
    /// `Database::list_local_song_paths_under` is already scoped to `source IS NULL`, so remote
    /// catalog rows from `add_source_song` are never candidates. A row is missing when
    /// `music_directory.join(path)` does not resolve to an existing regular file; a symlink
    /// counts as present only when the scanner follows symlinks, mirroring the walk in
    /// `collect_audio_files` (a row for a symlinked file is pruned by a scan configured not to
    /// follow them, as MPD does). Vanished rows are all deleted in a single transaction.
    fn prune_missing(&self, db: &Database, prefix: &str, stats: &mut ScanStats) {
        let music_dir = self
            .music_directory
            .as_ref()
            .expect("prune_missing is only called on a scanner with music_directory set");

        let paths = match db.list_local_song_paths_under(prefix) {
            Ok(paths) => paths,
            Err(e) => {
                warn!("failed to list local songs for prune: {}", e);
                stats.errors += 1;
                return;
            }
        };

        let missing: Vec<String> = paths
            .into_iter()
            .filter(|path| !self.file_is_present(music_dir.join(path).as_std_path()))
            .collect();

        if missing.is_empty() {
            return;
        }

        match db.delete_songs_by_paths(&missing) {
            Ok(deleted) => {
                stats.removed += deleted.len() as u32;
                for path in deleted {
                    debug!("pruned missing song: {}", path);
                    self.event_bus.emit(Event::SongDeleted { path });
                }
            }
            Err(e) => {
                warn!("failed to prune missing songs: {}", e);
                stats.errors += 1;
            }
        }
    }

    /// Delete directory rows that hold no songs and no child directories once their on-disk
    /// location is gone (e.g. `prune_missing` above just emptied it, or the whole directory was
    /// removed directly). Re-lists `Database::list_empty_directory_paths` after every pass:
    /// deleting a leaf can make its now-childless parent qualify on the next pass, so a vanished
    /// subtree collapses bottom-up within this one call. A row for a directory still present on
    /// disk is always kept even if empty, and a remote mount point's row is never a candidate —
    /// it holds remote songs, so it is never reported as empty.
    fn prune_empty_directories(&self, db: &Database, stats: &mut ScanStats) {
        let music_dir = self
            .music_directory
            .as_ref()
            .expect("prune_empty_directories is only called on a scanner with music_directory set");

        loop {
            let candidates = match db.list_empty_directory_paths() {
                Ok(paths) => paths,
                Err(e) => {
                    warn!("failed to list empty directories for prune: {}", e);
                    stats.errors += 1;
                    return;
                }
            };

            let mut pruned = 0u32;
            for path in candidates {
                if self.dir_is_present(music_dir.join(&path).as_std_path()) {
                    continue;
                }

                match db.delete_directory_by_path(&path) {
                    Ok(()) => {
                        pruned += 1;
                        debug!("pruned missing directory: {}", path);
                    }
                    Err(e) => {
                        warn!("failed to prune missing directory {}: {}", path, e);
                        stats.errors += 1;
                    }
                }
            }

            if pruned == 0 {
                break;
            }
        }
    }

    /// Whether a symlink at `path` should be followed, applying the split
    /// inside/outside policy (mpd `src/db/update/Config.cxx`): a target
    /// that resolves inside `music_directory` obeys `follow_inside_symlinks`,
    /// one that resolves outside obeys `follow_outside_symlinks`. Only
    /// meaningful when `path` is actually a symlink; callers check that first.
    fn should_follow_symlink(&self, path: &Path) -> bool {
        if self.follow_inside_symlinks && self.follow_outside_symlinks {
            return true;
        }
        if !self.follow_inside_symlinks && !self.follow_outside_symlinks {
            return false;
        }
        let Some(music_dir) = &self.music_directory else {
            return self.follow_outside_symlinks;
        };
        let Ok(target) = fs::canonicalize(path) else {
            // Dangling symlink: target can't be resolved, so it can't be
            // "inside" music_directory either.
            return self.follow_outside_symlinks;
        };
        let inside =
            fs::canonicalize(music_dir.as_std_path()).is_ok_and(|root| target.starts_with(root));
        if inside {
            self.follow_inside_symlinks
        } else {
            self.follow_outside_symlinks
        }
    }

    /// Whether `path` is excluded because it's a symlink the effective
    /// policy doesn't follow, mirroring the entry-skip in
    /// `collect_audio_files`.
    fn is_symlink_excluded(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
            && !self.should_follow_symlink(path)
    }

    /// Whether `path` is a regular file the scan would have visited: a
    /// symlink only counts when the effective policy follows it, like
    /// `collect_audio_files`.
    fn file_is_present(&self, path: &Path) -> bool {
        !self.is_symlink_excluded(path) && path.is_file()
    }

    /// Whether `path` is a directory the scan would have visited: a
    /// symlink only counts when the effective policy follows it, like
    /// `collect_audio_files`.
    fn dir_is_present(&self, path: &Path) -> bool {
        !self.is_symlink_excluded(path) && path.is_dir()
    }

    /// Convert absolute path to relative path (relative to music_directory)
    fn make_relative_path(&self, abs_path: &Utf8PathBuf) -> Result<Utf8PathBuf> {
        if let Some(music_dir) = &self.music_directory {
            // Strip music directory prefix
            if let Some(relative) = abs_path.as_str().strip_prefix(music_dir.as_str()) {
                let relative = relative.trim_start_matches('/');
                return Ok(Utf8PathBuf::from(relative));
            }
        }
        // Fallback: return as-is if we can't make it relative
        Ok(abs_path.clone())
    }

    fn scan_recursive(&self, db: &Database, path: &Path, stats: &mut ScanStats) -> Result<()> {
        // SOURCE ISOLATION: this scan only processes local filesystem files and
        // only calls `db.add_song()` (which never sets `source`). Any future
        // reconcile/prune step that deletes songs no longer on disk MUST use
        // `Database::delete_song_by_path` (which is guarded with `AND source IS NULL`)
        // or an equivalent query with a `WHERE source IS NULL` predicate to avoid
        // evicting remote catalog rows inserted by `Database::add_source_song`.

        // Step 1: Collect all audio files and their metadata (sequential directory walk).
        // `visited_dirs` tracks (dev, ino) pairs already recursed into, shared across the
        // whole tree walk, so a symlink cycle (or any other filesystem loop) can't cause
        // unbounded recursion regardless of the symlink-follow policy.
        let mut files_to_process = Vec::new();
        let mut visited_dirs = std::collections::HashSet::new();
        // Seed with the root itself so a symlink cycle that loops back to the
        // scan root (rather than to some deeper ancestor) is also detected.
        if let Ok(root_meta) = fs::metadata(path) {
            visited_dirs.insert((root_meta.dev(), root_meta.ino()));
        }
        self.collect_audio_files(
            db,
            path,
            &mut files_to_process,
            stats,
            &mut visited_dirs,
            &[],
        )?;

        // Step 2: Extract metadata in parallel
        let extracted: Vec<ExtractedMetadata> = files_to_process
            .into_par_iter()
            .map(|file_info| {
                match MetadataExtractor::extract_from_file(&file_info.absolute_path) {
                    Ok(mut song) => {
                        // Replace absolute path with relative path for storage
                        song.path = file_info.relative_path.clone();

                        // Embedded-cue container tracks: only worth checking FLAC
                        // files (the only format with an embedded-cue convention
                        // this fork supports — see `crate::embedded_cue`).
                        let container_tracks = if file_info
                            .absolute_path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("flac"))
                        {
                            crate::embedded_cue::read_embedded_cue_tracks(
                                &file_info.absolute_path,
                                song.duration,
                            )
                            .map(|tracks| {
                                crate::embedded_cue::build_container_tracks(&song, &tracks)
                            })
                            .unwrap_or_default()
                        } else {
                            Vec::new()
                        };

                        ExtractedMetadata {
                            file_info,
                            song: Some(song),
                            container_tracks,
                            error: None,
                        }
                    }
                    Err(e) => {
                        let error_msg = format!("{}", e);
                        ExtractedMetadata {
                            file_info,
                            song: None,
                            container_tracks: Vec::new(),
                            error: Some(error_msg),
                        }
                    }
                }
            })
            .collect();

        // Step 3: Batch insert into database (sequential, single connection).
        // Each `add_song` is itself several statements (upsert, tag delete, N tag
        // inserts, FTS delete+insert); autocommitting per-file makes a full-library
        // scan fsync thousands of times. Batch commits in chunks so one transaction
        // never holds an unbounded number of statements open.
        const TRANSACTION_BATCH_SIZE: usize = 500;
        let mut added = 0u32;
        let mut updated = 0u32;
        let mut errors = 0u32;

        for batch in extracted.chunks(TRANSACTION_BATCH_SIZE) {
            db.with_transaction(|db| {
                for extracted_meta in batch {
                    if let Some(error) = &extracted_meta.error {
                        warn!(
                            "failed to extract metadata from {}: {}",
                            extracted_meta.file_info.relative_path, error
                        );
                        errors += 1;
                        continue;
                    }

                    if let Some(song) = &extracted_meta.song {
                        match db.add_song_with_size(song, Some(extracted_meta.file_info.file_size))
                        {
                            Ok(_) => {
                                if let Err(e) = db.sync_container_tracks(
                                    song.path.as_str(),
                                    &extracted_meta.container_tracks,
                                ) {
                                    warn!(
                                        "failed to sync embedded-cue tracks for {}: {}",
                                        song.path, e
                                    );
                                    errors += 1;
                                }
                                let is_update = extracted_meta.file_info.existing_song.is_some();
                                if is_update {
                                    debug!("updated: {}", song.path);
                                    updated += 1;
                                } else {
                                    debug!("added: {}", song.path);
                                    added += 1;
                                }
                            }
                            Err(e) => {
                                warn!("failed to add {} to database: {}", song.path, e);
                                errors += 1;
                            }
                        }
                    }
                }
                Ok(())
            })?;
        }

        stats.added += added;
        stats.updated += updated;
        stats.errors += errors;

        Ok(())
    }

    /// Collect all audio files from the directory tree (sequential walk)
    fn collect_audio_files(
        &self,
        db: &Database,
        path: &Path,
        files: &mut Vec<FileInfo>,
        stats: &mut ScanStats,
        visited_dirs: &mut std::collections::HashSet<(u64, u64)>,
        inherited_patterns: &[IgnorePattern],
    ) -> Result<()> {
        let entries = fs::read_dir(path)
            .map_err(|e| RmpdError::Library(format!("Failed to read directory: {e}")))?;

        // `.mpdignore` patterns are inherited by subdirectories (mpd
        // `src/db/update/ExcludeList.cxx`), so this directory's effective
        // pattern set is its own file's patterns plus every ancestor's.
        let mut patterns = inherited_patterns.to_vec();
        patterns.extend(load_mpdignore(path));

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("failed to read directory entry: {}", e);
                    stats.errors += 1;
                    continue;
                }
            };

            let entry_path = entry.path();

            let file_name = entry_path.file_name().and_then(|n| n.to_str());

            // Skip hidden files and directories
            if let Some(name) = file_name
                && name.starts_with('.')
            {
                continue;
            }

            // Skip entries matched by a `.mpdignore` pattern (own or
            // inherited from an ancestor directory), mirroring mpd's
            // `UpdateDirectoryChild` exclude check (`src/db/update/Walk.cxx`).
            if let Some(name) = file_name
                && is_mpdignore_excluded(&patterns, name)
            {
                continue;
            }

            // A symlink is skipped unless the split inside/outside policy
            // says to follow it (mpd `src/db/update/Config.cxx`). Determine
            // whether this entry is itself a symlink first (DirEntry::file_type
            // never follows one, i.e. it's an lstat).
            let is_symlink = match entry.file_type() {
                Ok(ft) => ft.is_symlink(),
                Err(e) => {
                    warn!("failed to get file type for {:?}: {}", entry_path, e);
                    stats.errors += 1;
                    continue;
                }
            };
            if is_symlink && !self.should_follow_symlink(&entry_path) {
                continue;
            }

            // `entry.metadata()` never traverses a symlink (it's equivalent to `lstat`), so
            // a symlinked directory/file would otherwise be silently ignored even when
            // followed. Use `fs::metadata` (which follows symlinks, i.e. `stat`) for a
            // symlink we do follow, so `is_dir()`/`is_file()` reflect the link's target.
            let metadata = if is_symlink {
                fs::metadata(&entry_path)
            } else {
                entry.metadata()
            };

            let metadata = match metadata {
                Ok(m) => m,
                Err(e) => {
                    warn!("failed to read metadata for {:?}: {}", entry_path, e);
                    stats.errors += 1;
                    continue;
                }
            };

            if metadata.is_dir() {
                // Cycle guard: skip directories we've already recursed into (identified by
                // (dev, ino)). This catches symlink cycles (an ancestor pointing at itself
                // or a descendant) as well as any other hard/soft-link loop, regardless of
                // whether a symlink is followed.
                let dir_key = (metadata.dev(), metadata.ino());
                if !visited_dirs.insert(dir_key) {
                    warn!(
                        "skipping already-visited directory (symlink cycle?): {:?}",
                        entry_path
                    );
                    continue;
                }

                // Record directory with its filesystem mtime before recursing
                if let Ok(utf8_dir) = Utf8PathBuf::try_from(entry_path.clone())
                    && let Ok(rel_dir) = self.make_relative_path(&utf8_dir)
                {
                    let dir_mtime = system_time_to_unix_secs(
                        metadata
                            .modified()
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    );
                    if let Err(e) =
                        db.get_or_create_directory_with_mtime(rel_dir.as_path(), Some(dir_mtime))
                    {
                        warn!("failed to record directory {:?}: {}", entry_path, e);
                    }
                }
                // Recurse into subdirectory
                if let Err(e) =
                    self.collect_audio_files(db, &entry_path, files, stats, visited_dirs, &patterns)
                {
                    warn!("failed to scan directory {:?}: {}", entry_path, e);
                    stats.errors += 1;
                }
            } else if metadata.is_file() {
                // Convert to Utf8PathBuf
                let utf8_path = match Utf8PathBuf::try_from(entry_path.clone()) {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("skipping non-UTF8 path: {:?}", entry_path);
                        stats.errors += 1;
                        continue;
                    }
                };

                // Check if this is a supported audio file
                if !MetadataExtractor::is_supported_file(&utf8_path) {
                    continue;
                }

                stats.scanned += 1;

                // Emit progress every 100 files
                if stats.scanned.is_multiple_of(100) {
                    self.event_bus.emit(Event::DatabaseUpdateProgress {
                        scanned: stats.scanned,
                        total: 0, // Unknown total
                    });
                }

                // Convert to relative path for database storage
                let relative_path = match self.make_relative_path(&utf8_path) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("failed to convert path to relative: {}", e);
                        stats.errors += 1;
                        continue;
                    }
                };

                // Check if file already exists in database (using relative path)
                let existing_song = match db.get_song_by_path(relative_path.as_str()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("database error checking {}: {}", relative_path, e);
                        stats.errors += 1;
                        continue;
                    }
                };

                let mtime = system_time_to_unix_secs(
                    metadata
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                );
                let file_size = metadata.size();

                // Skip if file hasn't been modified (unless a forced rescan). A same/newer
                // mtime with a changed size (e.g. `cp -p`, a backup restore, some taggers
                // that preserve mtime) still counts as modified — mtime alone would miss it.
                if !self.force_rescan
                    && let Some(existing) = &existing_song
                    && existing.last_modified >= mtime
                {
                    let size_changed = match db.get_song_size_by_path(relative_path.as_str()) {
                        Ok(stored_size) => stored_size.is_some_and(|s| s != file_size),
                        Err(e) => {
                            warn!("database error checking size for {}: {}", relative_path, e);
                            false
                        }
                    };
                    if !size_changed {
                        continue;
                    }
                }

                // Add to files to process
                files.push(FileInfo {
                    absolute_path: utf8_path,
                    relative_path,
                    existing_song,
                    file_size,
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Default, Copy, Clone)]
pub struct ScanStats {
    pub scanned: u32,
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub errors: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use tempfile::TempDir;

    fn fixture_flac() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/samples/basic.flac")
    }

    #[test]
    fn glob_match_literal() {
        assert!(glob_match("foo.tmp", "foo.tmp"));
        assert!(!glob_match("foo.tmp", "foo.tmp2"));
        assert!(!glob_match("foo.tmp", "xfoo.tmp"));
    }

    #[test]
    fn glob_match_star_wildcard() {
        assert!(glob_match("*.tmp", "anything.tmp"));
        assert!(glob_match("*.tmp", ".tmp"));
        assert!(!glob_match("*.tmp", "anything.tmp2"));
        assert!(glob_match("a*b*c", "aXbYYc"));
        assert!(!glob_match("a*b*c", "aXbYY"));
    }

    #[test]
    fn glob_match_question_wildcard() {
        assert!(glob_match("track?.mp3", "track1.mp3"));
        assert!(!glob_match("track?.mp3", "track12.mp3"));
        assert!(!glob_match("track?.mp3", "track.mp3"));
    }

    #[test]
    fn glob_match_combined_wildcards() {
        assert!(glob_match("*.?lac", "cover.flac"));
        assert!(!glob_match("*.?lac", "cover.flacx"));
    }

    #[test]
    fn parse_mpdignore_skips_blank_and_comment_lines() {
        let content = "# comment\n\n*.tmp\n  # indented comment\n  *.bak  \n";
        assert_eq!(parse_mpdignore(content), vec!["*.tmp", "*.bak"]);
    }

    /// A `.mpdignore` at the music-directory root excludes matching files in
    /// every subdirectory (inheritance), while a pattern declared only in one
    /// subdirectory's own `.mpdignore` never leaks to a sibling directory —
    /// mirroring mpd's parent-chained `ExcludeList` (`src/db/update/ExcludeList.cxx`).
    #[test]
    fn mpdignore_patterns_are_inherited_by_subdirectories() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir).expect("create music dir");
        let fixture = fixture_flac();

        // Root .mpdignore: excludes any "ignored.flac", anywhere under music_dir.
        std::fs::write(music_dir.join(".mpdignore"), "ignored.flac\n").unwrap();
        std::fs::copy(&fixture, music_dir.join("keep.flac")).unwrap();

        let sub = music_dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::copy(&fixture, sub.join("ignored.flac")).unwrap();
        std::fs::copy(&fixture, sub.join("kept2.flac")).unwrap();

        // "other"'s own .mpdignore pattern must not leak to its sibling "other2".
        let other = music_dir.join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join(".mpdignore"), "local_only.flac\n").unwrap();
        std::fs::copy(&fixture, other.join("local_only.flac")).unwrap();

        let other2 = music_dir.join("other2");
        std::fs::create_dir(&other2).unwrap();
        std::fs::copy(&fixture, other2.join("local_only.flac")).unwrap();

        let db_path = temp_dir.path().join("test.db");
        let database = Database::open(db_path.to_str().unwrap()).expect("open database");
        let scanner = Scanner::new(EventBus::new(), false);

        let stats = scanner.scan_directory(&database, &music_dir).expect("scan");

        assert_eq!(
            stats.added, 3,
            "keep.flac, sub/kept2.flac, other2/local_only.flac"
        );
        assert!(database.get_song_by_path("keep.flac").unwrap().is_some());
        assert!(
            database
                .get_song_by_path("sub/kept2.flac")
                .unwrap()
                .is_some()
        );
        assert!(
            database
                .get_song_by_path("other2/local_only.flac")
                .unwrap()
                .is_some()
        );
        assert!(
            database
                .get_song_by_path("sub/ignored.flac")
                .unwrap()
                .is_none()
        );
        assert!(
            database
                .get_song_by_path("other/local_only.flac")
                .unwrap()
                .is_none()
        );
    }

    /// A symlink resolving inside `music_directory` obeys
    /// `follow_inside_symlinks`; one resolving outside obeys
    /// `follow_outside_symlinks`, independently of the other flag.
    #[test]
    fn should_follow_symlink_respects_inside_outside_split() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir).unwrap();
        let outside_dir = temp_dir.path().join("outside");
        std::fs::create_dir(&outside_dir).unwrap();

        let inside_target = music_dir.join("real.flac");
        std::fs::write(&inside_target, b"x").unwrap();
        let outside_target = outside_dir.join("real.flac");
        std::fs::write(&outside_target, b"x").unwrap();

        let inside_link = music_dir.join("inside_link");
        let outside_link = music_dir.join("outside_link");
        std::os::unix::fs::symlink(&inside_target, &inside_link).unwrap();
        std::os::unix::fs::symlink(&outside_target, &outside_link).unwrap();

        let music_utf8 = Utf8PathBuf::try_from(music_dir.clone()).unwrap();
        // Follow inside-pointing symlinks only.
        let scanner = Scanner::new(EventBus::new(), false)
            .with_symlink_policy(true, false)
            .with_music_dir(music_utf8);

        assert!(scanner.should_follow_symlink(&inside_link));
        assert!(!scanner.should_follow_symlink(&outside_link));
    }
}
