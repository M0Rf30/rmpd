use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, NoCache};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event as RmpdEvent, EventBus};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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
    debouncer: Option<Debouncer<RecommendedWatcher, NoCache>>,
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
        info!("Starting filesystem watcher for {:?}", self.music_dir);

        let (tx, mut rx) = mpsc::channel(EVENT_CHANNEL_SIZE);
        let db = Arc::clone(&self.db);
        let event_bus = self.event_bus.clone();
        let music_dir = self.music_dir.clone();

        // Create debouncer
        let debouncer = new_debouncer(
            DEBOUNCE_DURATION,
            None,
            move |result: DebounceEventResult| {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = tx.send(result).await {
                        error!("Failed to send watch event: {}", e);
                    }
                });
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
                                error!("Failed to handle filesystem event: {}", e);
                            }
                        }
                    }
                    Err(errors) => {
                        for error in errors {
                            error!("Filesystem watch error: {}", error);
                        }
                    }
                }
            }
        });

        info!("Filesystem watcher started successfully");
        Ok(())
    }

    /// Stop watching (graceful shutdown)
    pub fn stop(&mut self) {
        if self.debouncer.is_some() {
            info!("Stopping filesystem watcher");
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
        if let Some(name) = path.file_name() {
            if name.to_string_lossy().starts_with('.') {
                return false; // Skip hidden files
            }
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
                        debug!("Path outside music directory: {:?}", path);
                        continue;
                    }
                };

                let path_str = relative_path.to_string_lossy().to_string();

                debug!("File created/modified: {}", path_str);

                // Extract metadata
                let path_buf = camino::Utf8PathBuf::from(path.to_string_lossy().to_string());
                match MetadataExtractor::extract_from_file(&path_buf) {
                    Ok(song) => {
                        // Database operations need to be done with lock
                        let db_guard = match db.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => {
                                tracing::error!("Database mutex poisoned, recovering: {}", poisoned);
                                poisoned.into_inner()
                            }
                        };

                        // Check if song already exists
                        let exists = db_guard.get_song_by_path(&path_str)?.is_some();

                        // Add/update in database
                        db_guard.add_song(&song)?;

                        drop(db_guard); // Release lock before emitting event

                        // Emit appropriate event
                        if exists {
                            debug!("Song updated: {}", path_str);
                            event_bus.emit(RmpdEvent::SongUpdated(song));
                        } else {
                            debug!("Song added: {}", path_str);
                            event_bus.emit(RmpdEvent::SongAdded(song));
                        }
                    }
                    Err(e) => {
                        warn!("Failed to extract metadata from {}: {}", path_str, e);
                    }
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

                debug!("File removed: {}", path_str);

                // Remove from database
                let db_guard = match db.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        tracing::error!("Database mutex poisoned, recovering: {}", poisoned);
                        poisoned.into_inner()
                    }
                };
                db_guard.delete_song_by_path(&path_str)?;
                drop(db_guard);

                // Emit event
                event_bus.emit(RmpdEvent::SongDeleted {
                    path: path_str.clone(),
                });
            }
        }
        _ => {
            // Ignore other event types (access, metadata changes, etc.)
        }
    }

    Ok(())
}
