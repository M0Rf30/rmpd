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

    /// Returns `true` if any live source owns `scheme`.
    ///
    /// Used by add/addid command routing to decide whether to hand off to a
    /// remote source.
    pub fn is_source_scheme(&self, scheme: &str) -> bool {
        self.sources.iter().any(|s| s.scheme() == scheme)
    }

    /// Find the owning source for a virtual URI (`<scheme>://<name>/...`).
    ///
    /// Matches by `scheme()` **and** `name()` against the URI's scheme and
    /// authority components respectively. Returns `None` when no live source
    /// claims the URI.
    pub fn for_uri(&self, virtual_uri: &str) -> Option<&dyn MusicSource> {
        let (scheme, authority, _path) = parse_virtual_uri(virtual_uri)?;
        self.sources
            .iter()
            .find(|s| s.scheme() == scheme && s.name() == authority)
            .map(|s| s.as_ref())
    }

    /// Resolve a virtual URI to a directly-playable stream URL.
    ///
    /// Parses `<scheme>://<name>/<...>/<id>`, locates the owning source, and
    /// calls `source.resolve_stream_uri(id)` with the **trailing path segment**
    /// as the remote id.
    pub async fn resolve_stream_uri(&self, virtual_uri: &str) -> SourceResult<String> {
        let (scheme, authority, path) = parse_virtual_uri(virtual_uri).ok_or_else(|| {
            SourceError::NotFound(format!("malformed virtual URI: {virtual_uri}"))
        })?;
        let source = self
            .sources
            .iter()
            .find(|s| s.scheme() == scheme && s.name() == authority)
            .map(|s| s.as_ref())
            .ok_or_else(|| {
                SourceError::NotFound(format!("no source for {scheme}://{authority}"))
            })?;
        // The remote id is the trailing path segment.
        let id = path.rsplit('/').next().unwrap_or(path);
        source.resolve_stream_uri(id).await
    }

    /// Fetch cover-art bytes for a virtual URI, using the trailing path segment
    /// as the remote id. Returns `Ok(None)` when the URI is unowned or the
    /// source has no art for it.
    pub async fn cover_art(&self, virtual_uri: &str) -> SourceResult<Option<Vec<u8>>> {
        let (scheme, authority, path) = match parse_virtual_uri(virtual_uri) {
            Some(parts) => parts,
            None => return Ok(None),
        };
        let Some(source) = self
            .sources
            .iter()
            .find(|s| s.scheme() == scheme && s.name() == authority)
            .map(|s| s.as_ref())
        else {
            return Ok(None);
        };
        let id = path.rsplit('/').next().unwrap_or(path);
        source.cover_art(id).await
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

// ─── URI parsing helpers ──────────────────────────────────────────────────────

/// Parse `<scheme>://<authority>[/<path>]` into `(scheme, authority, path)`.
///
/// `path` is everything after the first `/` following the authority, or `""`
/// when the URI has no path component. No allocation; all slices borrow from
/// the input.
fn parse_virtual_uri(uri: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = uri.split_once("://")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if scheme.is_empty() || authority.is_empty() {
        return None;
    }
    Some((scheme, authority, path))
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
    fn for_uri_finds_filesystem_home() {
        let reg = make_registry_with_home();
        let source = reg.for_uri("file://home/x/y/id");
        assert!(source.is_some(), "should find a source for file://home/...");
        let s = source.unwrap();
        assert_eq!(s.scheme(), "file");
        assert_eq!(s.name(), "home");
    }

    #[test]
    fn for_uri_returns_none_for_wrong_name() {
        let reg = make_registry_with_home();
        assert!(
            reg.for_uri("file://other/x/y/id").is_none(),
            "wrong name should not match"
        );
    }

    #[test]
    fn for_uri_returns_none_for_wrong_scheme() {
        let reg = make_registry_with_home();
        assert!(
            reg.for_uri("subsonic://home/id").is_none(),
            "wrong scheme should not match"
        );
    }

    #[test]
    fn is_source_scheme_true_for_file() {
        let reg = make_registry_with_home();
        assert!(reg.is_source_scheme("file"));
        assert!(!reg.is_source_scheme("subsonic"));
    }

    #[test]
    fn parse_virtual_uri_splits_correctly() {
        let result = parse_virtual_uri("file://home/x/y/id");
        assert_eq!(result, Some(("file", "home", "x/y/id")));
    }

    #[test]
    fn parse_virtual_uri_no_path() {
        let result = parse_virtual_uri("file://home");
        assert_eq!(result, Some(("file", "home", "")));
    }

    #[test]
    fn parse_virtual_uri_rejects_malformed() {
        assert!(parse_virtual_uri("not-a-uri").is_none());
        assert!(parse_virtual_uri("://empty-scheme/foo").is_none());
    }
}
