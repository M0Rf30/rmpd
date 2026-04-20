use camino::Utf8PathBuf;
use rayon::prelude::*;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::event::{Event, EventBus};
use std::fs;
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
    #[allow(dead_code)] // Used by directory mtime comparison during incremental scans
    mtime: i64,
    existing_song: Option<rmpd_core::song::Song>,
}

/// Result of metadata extraction for a file
#[derive(Debug)]
struct ExtractedMetadata {
    file_info: FileInfo,
    song: Option<rmpd_core::song::Song>,
    error: Option<String>,
}

#[derive(Debug)]
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
        info!("starting music library scan: {}", root_path.display());
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
            "scan complete: {} files scanned, {} added, {} updated, {} errors",
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
        // Step 1: Collect all audio files and their metadata (sequential directory walk)
        let mut files_to_process = Vec::new();
        self.collect_audio_files(db, path, &mut files_to_process, stats)?;

        // Step 2: Extract metadata in parallel
        let extracted: Vec<ExtractedMetadata> = files_to_process
            .into_par_iter()
            .map(|file_info| {
                match MetadataExtractor::extract_from_file(&file_info.absolute_path) {
                    Ok(mut song) => {
                        // Replace absolute path with relative path for storage
                        song.path = file_info.relative_path.clone();
                        ExtractedMetadata {
                            file_info,
                            song: Some(song),
                            error: None,
                        }
                    }
                    Err(e) => {
                        let error_msg = format!("{}", e);
                        ExtractedMetadata {
                            file_info,
                            song: None,
                            error: Some(error_msg),
                        }
                    }
                }
            })
            .collect();

        // Step 3: Batch insert into database (sequential, single connection)
        let mut added = 0u32;
        let mut updated = 0u32;
        let mut errors = 0u32;

        for extracted_meta in extracted {
            if let Some(error) = extracted_meta.error {
                warn!(
                    "failed to extract metadata from {}: {}",
                    extracted_meta.file_info.relative_path, error
                );
                errors += 1;
                continue;
            }

            if let Some(song) = extracted_meta.song {
                match db.add_song(&song) {
                    Ok(_) => {
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
    ) -> Result<()> {
        let entries = fs::read_dir(path)
            .map_err(|e| RmpdError::Library(format!("Failed to read directory: {e}")))?;

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

            // Skip hidden files and directories
            if let Some(file_name) = entry_path.file_name().and_then(|n| n.to_str())
                && file_name.starts_with('.')
            {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    warn!("failed to read metadata for {:?}: {}", entry_path, e);
                    stats.errors += 1;
                    continue;
                }
            };

            if metadata.is_dir() {
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
                if let Err(e) = self.collect_audio_files(db, &entry_path, files, stats) {
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

                // Skip if file hasn't been modified
                if let Some(ref existing) = existing_song {
                    if existing.last_modified >= mtime {
                        continue;
                    }
                }

                // Add to files to process
                files.push(FileInfo {
                    absolute_path: utf8_path,
                    relative_path,
                    mtime,
                    existing_song,
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
    pub errors: u32,
}
