//! rmpd-source — music-source registry and backends.
//!
//! Provides a compile-time `SOURCE_PLUGINS` registry (see `registry.rs`),
//! the `SourceRegistry` runtime holder stored in `AppState`, and backend
//! implementations. Only `FilesystemSource` ships in PR1; `SubsonicSource`
//! is added in PR2 behind `feature = "subsonic"`.
//!
//! `sync_source` (PR5 catalog-sync integration) is intentionally absent here.

pub mod filesystem;
pub mod registry;
#[cfg(feature = "subsonic")]
pub mod subsonic;

// Re-export the SPI types so callers only need to depend on `rmpd-source`.
pub use registry::{SOURCE_PLUGINS, SourceFactory, create_source};
pub use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult};

use rmpd_core::config::SourceConfig;
use tracing::warn;

// ─── SourceRegistry ──────────────────────────────────────────────────────────

/// Runtime container for all live `MusicSource` instances, built from the
/// `[[source]]` config blocks at startup.
///
/// Stored in `AppState` (see PR3 wiring). Shared across Tokio tasks via
/// `Arc<RwLock<…>>` — the registry itself is `Send + Sync` because every
/// element is `Box<dyn MusicSource: Send + Sync>`.
pub struct SourceRegistry {
    pub sources: Vec<Box<dyn MusicSource>>,
}

impl SourceRegistry {
    /// Build a registry from a slice of `[[source]]` config blocks.
    ///
    /// Only `enabled` entries are kept. Construction failures are logged via
    /// `tracing::warn!` and skipped so a single bad config block does not
    /// abort startup.
    pub fn from_config(cfgs: &[SourceConfig]) -> Self {
        let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();
        for cfg in cfgs {
            if !cfg.enabled {
                continue;
            }
            match create_source(cfg) {
                Ok(source) => sources.push(source),
                Err(e) => warn!(
                    name = %cfg.name,
                    source_type = %cfg.source_type,
                    "failed to initialise source, skipping: {e}",
                ),
            }
        }
        Self { sources }
    }

    /// Iterate over all live sources.
    pub fn iter(&self) -> impl Iterator<Item = &dyn MusicSource> {
        self.sources.iter().map(|s| s.as_ref())
    }

    /// Find the source that owns `path` by matching the first `/`-segment (the
    /// mount point) against each source's [`name`](MusicSource::name).
    ///
    /// Mount-style virtual paths are `<name>/<artist>/<album>/<id>[.<suffix>]`
    /// with no `scheme://` prefix, mirroring how MPD surfaces mounted remote
    /// storage under a plain top-level directory. Returns `None` when no live
    /// source claims the path's mount point.
    pub fn owning_source(&self, path: &str) -> Option<&dyn MusicSource> {
        let mount = path.split('/').next().unwrap_or(path);
        if mount.is_empty() {
            return None;
        }
        self.sources
            .iter()
            .find(|s| s.name() == mount)
            .map(|s| s.as_ref())
    }

    /// `true` when a live source owns `path` (mount-point match). Convenience
    /// over [`owning_source`](Self::owning_source) for command routing.
    pub fn owns_path(&self, path: &str) -> bool {
        self.owning_source(path).is_some()
    }

    /// Resolve a mount-style virtual path to a directly-playable stream URL.
    ///
    /// Locates the owning source via the first segment, recovers the remote id
    /// from the last segment (stripping a trailing audio extension), and calls
    /// `source.resolve_stream_uri(id)`.
    pub async fn resolve_stream_uri(&self, path: &str) -> SourceResult<String> {
        let source = self
            .owning_source(path)
            .ok_or_else(|| SourceError::NotFound(format!("no source owns path: {path}")))?;
        source.resolve_stream_uri(extract_remote_id(path)).await
    }

    /// Fetch cover-art bytes for a mount-style virtual path, using the remote
    /// id recovered from the last segment. Returns `Ok(None)` when the path is
    /// unowned or the source has no art for it.
    pub async fn cover_art(&self, path: &str) -> SourceResult<Option<Vec<u8>>> {
        let Some(source) = self.owning_source(path) else {
            return Ok(None);
        };
        source.cover_art(extract_remote_id(path)).await
    }
    /// Number of live sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// True when no live sources are registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// Sync a music source's catalog into the local database.
///
/// Calls `source.list_all()`, then atomically replaces every cached row for
/// that source in one `spawn_blocking` transaction: `clear_source` followed by
/// `add_source_song` for each song. Returns the number of songs inserted.
///
/// This function is `async` because `list_all` does network I/O; the DB work
/// runs on a blocking thread so libsqlite does not stall the Tokio runtime.
pub async fn sync_source(source: &dyn MusicSource, db_path: &str) -> Result<usize, SourceError> {
    let songs = source.list_all().await?;
    let count = songs.len();
    let token = format!("{}:{}", source.scheme(), source.name());
    let db_path = db_path.to_owned();
    tokio::task::spawn_blocking(move || -> Result<usize, SourceError> {
        let db = rmpd_library::Database::open(&db_path)
            .map_err(|e| SourceError::Protocol(format!("failed to open database: {e}")))?;
        let _old = db
            .clear_source(&token)
            .map_err(|e| SourceError::Protocol(format!("clear_source: {e}")))?;
        for song in &songs {
            db.add_source_song(song, &token)
                .map_err(|e| SourceError::Protocol(format!("add_source_song: {e}")))?;
        }
        Ok(count)
    })
    .await
    .map_err(|e| SourceError::Protocol(format!("sync task panicked: {e}")))?
}

// ─── Path helpers ──────────────────────────────────────────────────────────────

/// Known audio-file extensions appended to a virtual leaf by a source's song
/// mapper (e.g. Subsonic's `Child.suffix`). Compared case-insensitively when
/// recovering the bare remote id from a mount-style path.
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "oga", "opus", "m4a", "aac", "mp4", "wav", "wv", "ape", "wma", "alac",
    "aif", "aiff", "dsf", "dff",
];

/// Recover the raw remote id from a mount-style path's last `/`-segment by
/// stripping a trailing known audio extension (case-insensitive).
///
/// `map_song` builds the leaf as `<id>[.<suffix>]`; the id itself is never
/// encoded and Subsonic ids are opaque tokens, so removing a recognized audio
/// extension yields the exact id the backend expects. A leaf without such an
/// extension (no suffix was appended) is returned unchanged.
fn extract_remote_id(path: &str) -> &str {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if let Some((stem, ext)) = leaf.rsplit_once('.')
        && AUDIO_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext))
    {
        return stem;
    }
    leaf
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::FilesystemSource;
    use camino::Utf8PathBuf;

    fn make_registry_with_home() -> SourceRegistry {
        let source = Box::new(FilesystemSource {
            name: "home".to_owned(),
            music_dir: Utf8PathBuf::from("/music"),
            db_path: "/tmp/test.db".to_owned(),
        }) as Box<dyn MusicSource>;
        SourceRegistry {
            sources: vec![source],
        }
    }

    #[test]
    fn owning_source_matches_first_segment() {
        let reg = make_registry_with_home();
        let source = reg.owning_source("home/Artist/Album/id.flac");
        assert!(source.is_some(), "should own a path under the 'home' mount");
        assert_eq!(source.unwrap().name(), "home");
    }

    #[test]
    fn owning_source_none_for_unowned_mount() {
        let reg = make_registry_with_home();
        assert!(reg.owning_source("other/Artist/Album/id").is_none());
        // A bare radio URI's first segment ("http:") never matches a mount name.
        assert!(reg.owning_source("http://radio.example/stream").is_none());
        assert!(reg.owning_source("").is_none());
    }

    #[test]
    fn owns_path_is_owning_source_predicate() {
        let reg = make_registry_with_home();
        assert!(reg.owns_path("home/a/b/c.mp3"));
        assert!(!reg.owns_path("Music/a/b/c.mp3"));
    }

    #[test]
    fn extract_remote_id_strips_known_audio_extension() {
        // Extension stripped, case-insensitively.
        assert_eq!(extract_remote_id("home/A/B/song-123.flac"), "song-123");
        assert_eq!(extract_remote_id("home/A/B/song-123.FLAC"), "song-123");
        assert_eq!(extract_remote_id("home/A/B/al-7.opus"), "al-7");
        // No extension: returned unchanged (no suffix was appended).
        assert_eq!(extract_remote_id("home/A/B/song-123"), "song-123");
        // Unknown extension is NOT stripped (ids may contain dots).
        assert_eq!(extract_remote_id("home/A/B/id.42"), "id.42");
        // Bare leaf with no separators.
        assert_eq!(extract_remote_id("song-123.mp3"), "song-123");
    }
}
