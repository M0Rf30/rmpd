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

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{ModifyKind, RenameMode};
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
}
