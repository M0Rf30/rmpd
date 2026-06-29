//! `FilesystemSource` — wraps `rmpd_library::Database` via `spawn_blocking`.
//!
//! Local playback never routes through this trait (`resolve_stream_uri` returns
//! `NotFound`); the filesystem source exists so that `update`/`list_all` and
//! browse commands share the same `MusicSource` abstraction as remote backends.

use async_trait::async_trait;
use camino::Utf8PathBuf;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use rmpd_library::Database;
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult};

// ─── Struct ──────────────────────────────────────────────────────────────────

pub struct FilesystemSource {
    pub(crate) name: String,
    pub(crate) music_dir: Utf8PathBuf,
    /// Path to the SQLite database file (passed to `Database::open`).
    pub(crate) db_path: String,
}

// ─── Factory ─────────────────────────────────────────────────────────────────

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS`.
pub fn filesystem_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let music_dir = cfg.setting_str("music_directory").ok_or_else(|| {
        SourceError::Config("filesystem source requires a `music_directory` setting".to_owned())
    })?;
    let db_path = cfg.setting_str("db").ok_or_else(|| {
        SourceError::Config("filesystem source requires a `db` setting".to_owned())
    })?;
    Ok(Box::new(FilesystemSource {
        name: cfg.name.clone(),
        music_dir: Utf8PathBuf::from(music_dir),
        db_path,
    }))
}

// ─── MusicSource impl ────────────────────────────────────────────────────────

#[async_trait]
impl MusicSource for FilesystemSource {
    fn scheme(&self) -> &str {
        "file"
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Checks that `music_dir` exists and is a directory.
    async fn ping(&self) -> SourceResult<()> {
        let dir = self.music_dir.clone();
        tokio::task::spawn_blocking(move || match std::fs::metadata(dir.as_str()) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(SourceError::Unreachable(format!(
                "music_directory exists but is not a directory: {dir}"
            ))),
            Err(e) => Err(SourceError::Unreachable(format!(
                "music_directory not accessible ({e}): {dir}"
            ))),
        })
        .await
        .map_err(|e| SourceError::Protocol(format!("spawn_blocking panicked: {e}")))?
    }

    /// Lists immediate children of a virtual directory path.
    ///
    /// Delegates to `Database::list_directory` inside `spawn_blocking`.
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        let db_path = self.db_path.clone();
        let dir = dir.to_owned();
        tokio::task::spawn_blocking(move || {
            let db = Database::open(&db_path).map_err(|e| SourceError::Protocol(e.to_string()))?;
            let listing = db
                .list_directory(&dir)
                .map_err(|e| SourceError::Protocol(e.to_string()))?;
            let mut entries: Vec<SourceEntry> =
                Vec::with_capacity(listing.directories.len() + listing.songs.len());
            for (path, _mtime) in listing.directories {
                entries.push(SourceEntry::Dir(path));
            }
            for song in listing.songs {
                entries.push(SourceEntry::Song(song));
            }
            Ok(entries)
        })
        .await
        .map_err(|e| SourceError::Protocol(format!("spawn_blocking panicked: {e}")))?
    }

    /// Returns every song in the catalog — used by the `update` path.
    ///
    /// Delegates to `Database::list_all_songs` inside `spawn_blocking`.
    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let db = Database::open(&db_path).map_err(|e| SourceError::Protocol(e.to_string()))?;
            db.list_all_songs()
                .map_err(|e| SourceError::Protocol(e.to_string()))
        })
        .await
        .map_err(|e| SourceError::Protocol(format!("spawn_blocking panicked: {e}")))?
    }

    /// Server-side search — delegates to `Database::search_songs`.
    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let db_path = self.db_path.clone();
        let query = query.to_owned();
        tokio::task::spawn_blocking(move || {
            let db = Database::open(&db_path).map_err(|e| SourceError::Protocol(e.to_string()))?;
            db.search_songs(&query)
                .map_err(|e| SourceError::Protocol(e.to_string()))
        })
        .await
        .map_err(|e| SourceError::Protocol(format!("spawn_blocking panicked: {e}")))?
    }

    /// Always returns `NotFound`: local songs play via the direct file path
    /// (`resolve_path` → `File::open`), never through this trait.
    async fn resolve_stream_uri(&self, _song_id: &str) -> SourceResult<String> {
        Err(SourceError::NotFound(
            "filesystem songs play via the direct file path, not a stream URI".to_owned(),
        ))
    }
}
