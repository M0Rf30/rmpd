use camino::Utf8PathBuf;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event, EventBus};
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::metadata::MetadataExtractor;

pub struct Scanner {
    event_bus: EventBus,
    music_directory: Option<Utf8PathBuf>,
}

impl Scanner {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            music_directory: None,
        }
    }

    pub fn scan_directory(&self, db: &Database, root_path: &Path) -> Result<ScanStats> {
        info!("Starting music library scan: {}", root_path.display());
        self.event_bus.emit(Event::DatabaseUpdateStarted);

        let mut stats = ScanStats::default();

        // Store music directory for relative path conversion
        let music_dir = Utf8PathBuf::try_from(root_path.to_path_buf())
            .map_err(|_| RmpdError::Library("Music directory path is not valid UTF-8".into()))?;

        // Create a scanner with music_directory set
        let scanner_with_dir = Scanner {
            event_bus: self.event_bus.clone(),
            music_directory: Some(music_dir),
        };

        scanner_with_dir.scan_recursive(db, root_path, &mut stats)?;

        info!(
            "Scan complete: {} files scanned, {} added, {} updated, {} errors",
            stats.scanned, stats.added, stats.updated, stats.errors
        );

        self.event_bus.emit(Event::DatabaseUpdateFinished);

        Ok(stats)
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
        let entries = fs::read_dir(path)
            .map_err(|e| RmpdError::Library(format!("Failed to read directory: {}", e)))?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("Failed to read directory entry: {}", e);
                    stats.errors += 1;
                    continue;
                }
            };

            let entry_path = entry.path();

            // Skip hidden files and directories
            if let Some(file_name) = entry_path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with('.') {
                    continue;
                }
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    warn!("Failed to read metadata for {:?}: {}", entry_path, e);
                    stats.errors += 1;
                    continue;
                }
            };

            if metadata.is_dir() {
                // Recurse into subdirectory
                if let Err(e) = self.scan_recursive(db, &entry_path, stats) {
                    warn!("Failed to scan directory {:?}: {}", entry_path, e);
                    stats.errors += 1;
                }
            } else if metadata.is_file() {
                // Convert to Utf8PathBuf
                let utf8_path = match Utf8PathBuf::try_from(entry_path.clone()) {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Skipping non-UTF8 path: {:?}", entry_path);
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
                if stats.scanned % 100 == 0 {
                    self.event_bus.emit(Event::DatabaseUpdateProgress {
                        scanned: stats.scanned,
                        total: 0, // Unknown total
                    });
                }

                // Convert to relative path for database storage
                let relative_path = match self.make_relative_path(&utf8_path) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to convert path to relative: {}", e);
                        stats.errors += 1;
                        continue;
                    }
                };

                // Check if file already exists in database (using relative path)
                let existing = match db.get_song_by_path(relative_path.as_str()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Database error checking {}: {}", relative_path, e);
                        stats.errors += 1;
                        continue;
                    }
                };

                let mtime = metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;

                // Skip if file hasn't been modified
                let is_update = if let Some(ref existing_song) = existing {
                    if existing_song.last_modified >= mtime {
                        debug!("Skipping unchanged file: {}", relative_path);
                        continue;
                    }
                    true
                } else {
                    false
                };

                // Extract metadata using absolute path for reading, but store relative path
                match MetadataExtractor::extract_from_file(&utf8_path) {
                    Ok(mut song) => {
                        // Replace absolute path with relative path for storage
                        song.path = relative_path.clone();

                        match db.add_song(&song) {
                            Ok(_) => {
                                if is_update {
                                    debug!("Updated: {}", relative_path);
                                    stats.updated += 1;
                                } else {
                                    debug!("Added: {}", relative_path);
                                    stats.added += 1;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to add {} to database: {}", relative_path, e);
                                stats.errors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to extract metadata from {}: {}", relative_path, e);
                        stats.errors += 1;
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    pub scanned: u32,
    pub added: u32,
    pub updated: u32,
    pub errors: u32,
}
