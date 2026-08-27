use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event as RmpdEvent, EventBus};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::database::Database;
use crate::metadata::MetadataExtractor;

const DEBOUNCE_DURATION: Duration = Duration::from_millis(300);
const EVENT_CHANNEL_SIZE: usize = 1024;

pub struct FilesystemWatcher {
    music_dir: PathBuf,
    db: Arc<Mutex<Database>>,
    event_bus: EventBus,
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl fmt::Debug for FilesystemWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilesystemWatcher")
            .field("music_dir", &self.music_dir)
            .field("event_bus", &self.event_bus)
            .field("debouncer_active", &self.debouncer.is_some())
            .finish_non_exhaustive()
    }
}

impl FilesystemWatcher {
    pub fn new(music_dir: PathBuf, db: Arc<Mutex<Database>>, event_bus: EventBus) -> Result<Self> {
        Ok(Self {
            music_dir,
            db,
            event_bus,
            debouncer: None,
        })
    }

    /// Start watching the music directory
    pub async fn start(&mut self) -> Result<()> {
        info!("starting filesystem watcher for {:?}", self.music_dir);

        let (tx, mut rx) = mpsc::channel(EVENT_CHANNEL_SIZE);
        let db = Arc::clone(&self.db);
        let event_bus = self.event_bus.clone();
        let music_dir = self.music_dir.clone();

        // Create debouncer
        let debouncer = new_debouncer(
            DEBOUNCE_DURATION,
            None,
            move |result: DebounceEventResult| {
                // This callback runs on notify's own dedicated thread, which has
                // no Tokio runtime — so we must NOT `tokio::spawn` here (that
                // panics with "no reactor running"). `blocking_send` bridges the
                // event into the async handler task below.
                if let Err(e) = tx.blocking_send(result) {
                    error!("failed to send watch event: {}", e);
                }
            },
        )
        .map_err(|e| RmpdError::Library(format!("Failed to create watcher: {e}")))?;

        // Watch the music directory recursively
        let mut watcher = debouncer;
        watcher
            .watch(&self.music_dir, RecursiveMode::Recursive)
            .map_err(|e| RmpdError::Library(format!("Failed to watch directory: {e}")))?;

        self.debouncer = Some(watcher);

        // Emit start event
        self.event_bus.emit(RmpdEvent::FilesystemWatchStarted);

        // Spawn event handler task
        tokio::spawn(async move {
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(events) => {
                        for event in events {
                            if let Err(e) =
                                handle_fs_event(&event, &music_dir, &db, &event_bus).await
                            {
                                error!("failed to handle filesystem event: {}", e);
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            error!("filesystem watch error: {}", error);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop watching (graceful shutdown)
    pub fn stop(&mut self) {
        if self.debouncer.is_some() {
            info!("stopping filesystem watcher");
            self.debouncer = None;
            self.event_bus.emit(RmpdEvent::FilesystemWatchStopped);
        }
    }
}

impl Drop for FilesystemWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn handle_fs_event(
    event: &Event,
    music_dir: &Path,
    db: &Arc<Mutex<Database>>,
    event_bus: &EventBus,
) -> Result<()> {
    // Filter out non-audio files and hidden files
    let is_audio_file = |path: &Path| -> bool {
        if let Some(name) = path.file_name()
            && name.to_string_lossy().starts_with('.')
        {
            return false; // Skip hidden files
        }

        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_lowercase().as_str(),
                    "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "wv" | "ape" | "mpc"
                )
            })
            .unwrap_or(false)
    };

    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in &event.paths {
                if !is_audio_file(path) {
                    prune_if_vanished_directory(path, music_dir, db, event_bus).await?;
                    continue;
                }

                // Make path relative to music directory
                let relative_path = match path.strip_prefix(music_dir) {
                    Ok(p) => p,
                    Err(_) => {
                        debug!("path outside music directory: {:?}", path);
                        continue;
                    }
                };

                let path_str = relative_path.to_string_lossy().to_string();

                // `notify` reports a rename/move-away as a modify (sometimes a
                // create for the source path), not a remove. If the path is no
                // longer a regular file (gone, or replaced by a directory),
                // treat it exactly like `Remove`.
                if !path.is_file() {
                    debug!("file moved away: {}", path_str);
                    remove_song_row(db, event_bus, &path_str).await?;
                    continue;
                }

                debug!("file created/modified: {}", path_str);

                // Extract metadata off the async runtime: file I/O and tag parsing block.
                let path_buf = camino::Utf8PathBuf::from(path.to_string_lossy().to_string());
                let extraction = tokio::task::spawn_blocking(move || {
                    MetadataExtractor::extract_from_file(&path_buf)
                })
                .await;

                let mut song = match extraction {
                    Ok(Ok(song)) => song,
                    Ok(Err(e)) => {
                        // The file may have vanished between the `is_file` check
                        // above and the extraction: then it is a removal, not an
                        // extraction failure.
                        if !path.is_file() {
                            debug!("file moved away during extraction: {}", path_str);
                            remove_song_row(db, event_bus, &path_str).await?;
                            continue;
                        }
                        warn!("failed to extract metadata from {}: {}", path_str, e);
                        continue;
                    }
                    Err(e) => {
                        warn!("metadata extraction task panicked for {}: {}", path_str, e);
                        continue;
                    }
                };

                // Store the same music-dir-relative path the scanner uses, so
                // get_song_by_path lookups (lsinfo/add/playlistinfo/stickers) find it.
                song.path = camino::Utf8PathBuf::from(path_str.clone());

                // Database operations need to be done with lock
                let db_guard = db.lock().await;

                // Check if song already exists
                let exists = db_guard.get_song_by_path(&path_str)?.is_some();

                // Add/update in database
                db_guard.add_song(&song)?;

                drop(db_guard); // Release lock before emitting event

                // Emit appropriate event
                if exists {
                    debug!("song updated: {}", path_str);
                    event_bus.emit(RmpdEvent::SongUpdated(song));
                } else {
                    debug!("song added: {}", path_str);
                    event_bus.emit(RmpdEvent::SongAdded(song));
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &event.paths {
                if !is_audio_file(path) {
                    prune_if_vanished_directory(path, music_dir, db, event_bus).await?;
                    continue;
                }

                let relative_path = match path.strip_prefix(music_dir) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let path_str = relative_path.to_string_lossy().to_string();

                debug!("file removed: {}", path_str);

                remove_song_row(db, event_bus, &path_str).await?;
            }
        }
        _ => {
            // Ignore other event types (access, metadata changes, etc.)
        }
    }

    Ok(())
}

/// Delete a song's row and emit `SongDeleted`. Shared by the `Remove` branch
/// and the `Create | Modify` branch's vanished-path check, so a file that
/// disappears from disk is handled identically regardless of which `notify`
/// event kind reported it.
async fn remove_song_row(
    db: &Arc<Mutex<Database>>,
    event_bus: &EventBus,
    path_str: &str,
) -> Result<()> {
    let db_guard = db.lock().await;
    db_guard.delete_song_by_path(path_str)?;
    drop(db_guard);

    event_bus.emit(RmpdEvent::SongDeleted {
        path: path_str.to_string(),
    });

    Ok(())
}

/// If `path` is not an audio file (the caller already checked that) and no
/// longer exists on disk, it denotes a directory (or some other non-audio
/// path) that was removed or moved away out from under the watcher: `notify`
/// reports the event on the directory path itself, not on each audio file
/// inside it, so none of those files' own `Remove`/`Modify` events ever fire
/// and `is_audio_file` never sees them. Prune every local row under it.
/// A path that still exists (e.g. a directory that is untouched, or some
/// other non-audio file) is left alone — the caller already skips it.
/// If `path` *is* the music directory itself, `strip_prefix` yields `""`,
/// and pruning `""` would delete every local row in the library — but the
/// music directory can be a transiently-unmounted mount point, so a watcher
/// event on it is not proof the whole library is gone. Refuse to prune in
/// that case instead of trusting it.
async fn prune_if_vanished_directory(
    path: &Path,
    music_dir: &Path,
    db: &Arc<Mutex<Database>>,
    event_bus: &EventBus,
) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    let relative_path = match path.strip_prefix(music_dir) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let rel_str = relative_path.to_string_lossy().to_string();

    if rel_str.is_empty() {
        warn!(
            "music directory {:?} vanished; refusing to prune the entire library",
            music_dir
        );
        return Ok(());
    }

    debug!("directory vanished, pruning rows under: {}", rel_str);
    remove_rows_under(db, event_bus, &rel_str).await
}

/// Delete every local row whose path is `rel` itself or starts with `rel/`,
/// in one lock acquisition and one transaction, then emit `SongDeleted` for
/// each row actually deleted — used when `rel` denotes a directory that
/// vanished from disk. `list_local_song_paths_under` and
/// `delete_songs_by_paths` are both already scoped to local rows
/// (`source IS NULL`), so remote catalog rows are never touched. A `rel`
/// under which nothing matches (e.g. a non-audio file that vanished) is a
/// no-op.
async fn remove_rows_under(
    db: &Arc<Mutex<Database>>,
    event_bus: &EventBus,
    rel: &str,
) -> Result<()> {
    let deleted = {
        let db_guard = db.lock().await;
        let paths = db_guard.list_local_song_paths_under(rel)?;
        db_guard.delete_songs_by_paths(&paths)?
    };

    for path in deleted {
        event_bus.emit(RmpdEvent::SongDeleted { path });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{ModifyKind, RemoveKind, RenameMode};
    use rmpd_core::song::Song;

    fn local_song(path: &str) -> Song {
        Song {
            id: 0,
            path: path.into(),
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

    /// A rename/move-away arrives from `notify` as a modify event for a path
    /// that no longer exists (backends differ; inotify reports `MOVED_FROM`).
    /// Independently of the backend, that event must delete the row and emit
    /// `SongDeleted`, exactly like a `Remove` would.
    #[tokio::test]
    async fn modify_event_for_a_vanished_path_removes_the_row() {
        let temp_dir = tempfile::TempDir::new().expect("create temp dir");
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir).expect("create music dir");
        let db_path = temp_dir.path().join("test.db");
        let database = Database::open(db_path.to_str().unwrap()).expect("open database");
        database
            .add_song(&local_song("song.flac"))
            .expect("insert the row the file used to have");
        let db = Arc::new(Mutex::new(database));
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe();

        // The file was never created on disk: the path no longer exists.
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
            .add_path(music_dir.join("song.flac"));
        handle_fs_event(&event, &music_dir, &db, &event_bus)
            .await
            .expect("handle the synthetic event");

        assert!(
            db.lock()
                .await
                .get_song_by_path("song.flac")
                .expect("query")
                .is_none(),
            "the row of a vanished path must be deleted"
        );
        match rx.try_recv() {
            Ok(RmpdEvent::SongDeleted { path }) => assert_eq!(path, "song.flac"),
            other => panic!("expected SongDeleted for song.flac, got {other:?}"),
        }
    }

    /// Shared body for the two vanished-directory tests below: seed `dir/a.flac`,
    /// `dir/b.flac` and a control row `other/c.flac`, feed `handle_fs_event` a
    /// synthetic event of `kind` whose path is `music_dir/dir` (a directory that
    /// does not exist on disk, since nothing was ever created there), and assert
    /// both rows under `dir/` are gone, the control row survives, and a
    /// `SongDeleted` was emitted for each pruned row.
    async fn assert_vanished_directory_prunes_rows_under_it(kind: EventKind) {
        let temp_dir = tempfile::TempDir::new().expect("create temp dir");
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir).expect("create music dir");
        let db_path = temp_dir.path().join("test.db");
        let database = Database::open(db_path.to_str().unwrap()).expect("open database");
        database
            .add_song(&local_song("dir/a.flac"))
            .expect("insert dir/a.flac");
        database
            .add_song(&local_song("dir/b.flac"))
            .expect("insert dir/b.flac");
        database
            .add_song(&local_song("other/c.flac"))
            .expect("insert control row other/c.flac");
        let db = Arc::new(Mutex::new(database));
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe();

        // `dir` was never created on disk: the directory no longer exists.
        let event = Event::new(kind).add_path(music_dir.join("dir"));
        handle_fs_event(&event, &music_dir, &db, &event_bus)
            .await
            .expect("handle the synthetic event");

        let db_guard = db.lock().await;
        assert!(
            db_guard
                .get_song_by_path("dir/a.flac")
                .expect("query")
                .is_none(),
            "dir/a.flac should be pruned along with the vanished directory"
        );
        assert!(
            db_guard
                .get_song_by_path("dir/b.flac")
                .expect("query")
                .is_none(),
            "dir/b.flac should be pruned along with the vanished directory"
        );
        assert!(
            db_guard
                .get_song_by_path("other/c.flac")
                .expect("query")
                .is_some(),
            "other/c.flac is outside the vanished directory and must survive"
        );
        drop(db_guard);

        let mut deleted_paths = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                RmpdEvent::SongDeleted { path } => deleted_paths.push(path),
                other => panic!("expected only SongDeleted events, got {other:?}"),
            }
        }
        deleted_paths.sort();
        assert_eq!(
            deleted_paths,
            vec!["dir/a.flac".to_string(), "dir/b.flac".to_string()],
            "a SongDeleted event should be emitted for each pruned row"
        );
    }

    /// A directory removed or moved away arrives from `notify` as a modify/rename
    /// event on the directory's own path (backends differ; inotify reports
    /// `MOVED_FROM`), not one event per file inside it — `is_audio_file` never
    /// matches a directory, so without special handling every row under it would
    /// be left behind (issue #12 follow-up).
    #[tokio::test]
    async fn modify_event_for_a_vanished_directory_prunes_rows_under_it() {
        assert_vanished_directory_prunes_rows_under_it(EventKind::Modify(ModifyKind::Name(
            RenameMode::From,
        )))
        .await;
    }

    /// Same as above, but for the `Remove` event kind `notify` reports when a
    /// watched directory is deleted outright (e.g. `IN_DELETE_SELF` on inotify).
    #[tokio::test]
    async fn remove_event_for_a_vanished_directory_prunes_rows_under_it() {
        assert_vanished_directory_prunes_rows_under_it(EventKind::Remove(RemoveKind::Folder)).await;
    }

    /// A watcher event on the music directory path itself must never wipe the
    /// whole library: `strip_prefix` yields `""` for that path, which without
    /// the guard in `prune_if_vanished_directory` would match every local row.
    #[tokio::test]
    async fn modify_event_for_the_music_directory_itself_leaves_all_rows() {
        let temp_dir = tempfile::TempDir::new().expect("create temp dir");
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir).expect("create music dir");
        let db_path = temp_dir.path().join("test.db");
        let database = Database::open(db_path.to_str().unwrap()).expect("open database");
        database
            .add_song(&local_song("dir/a.flac"))
            .expect("insert dir/a.flac");
        let db = Arc::new(Mutex::new(database));
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe();

        // Simulate the music directory itself vanishing (e.g. an unmounted
        // mount point): the event path is the music directory, so its
        // relative path is "".
        std::fs::remove_dir(&music_dir).expect("remove music dir to simulate vanish");
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
            .add_path(music_dir.clone());
        handle_fs_event(&event, &music_dir, &db, &event_bus)
            .await
            .expect("handle the synthetic event");

        assert!(
            db.lock()
                .await
                .get_song_by_path("dir/a.flac")
                .expect("query")
                .is_some(),
            "a vanished music directory must not wipe the library"
        );
        assert!(
            rx.try_recv().is_err(),
            "no SongDeleted should be emitted when refusing to prune the whole library"
        );
    }
}
