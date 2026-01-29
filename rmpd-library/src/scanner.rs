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
}

impl Scanner {
    pub fn new(event_bus: EventBus) -> Self {
        Self { event_bus }
    }

    pub fn scan_directory(&self, db: &Database, root_path: &Path) -> Result<ScanStats> {
        info!("Starting music library scan: {}", root_path.display());
        self.event_bus.emit(Event::DatabaseUpdateStarted);

        let mut stats = ScanStats::default();

        self.scan_recursive(db, root_path, &mut stats)?;

        info!(
            "Scan complete: {} files scanned, {} added, {} updated, {} errors",
            stats.scanned, stats.added, stats.updated, stats.errors
        );

        self.event_bus.emit(Event::DatabaseUpdateFinished);

        Ok(stats)
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
            if entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
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

                // Check if file already exists in database
                let existing = match db.get_song_by_path(utf8_path.as_str()) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Database error checking {}: {}", utf8_path, e);
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
                        debug!("Skipping unchanged file: {}", utf8_path);
                        continue;
                    }
                    true
                } else {
                    false
                };

                // Extract metadata and add/update in database
                match MetadataExtractor::extract_from_file(&utf8_path) {
                    Ok(song) => {
                        match db.add_song(&song) {
                            Ok(_) => {
                                if is_update {
                                    debug!("Updated: {}", utf8_path);
                                    stats.updated += 1;
                                } else {
                                    debug!("Added: {}", utf8_path);
                                    stats.added += 1;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to add {} to database: {}", utf8_path, e);
                                stats.errors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to extract metadata from {}: {}", utf8_path, e);
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
