// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! rmpd-source — music-source registry and backends.
//!
//! Provides a compile-time `SOURCE_PLUGINS` registry (see `registry.rs`),
//! the `SourceRegistry` runtime holder stored in `AppState`, and backend
//! implementations. Only `FilesystemSource` ships in PR1; `SubsonicSource`
//! is added in PR2 behind `feature = "subsonic"`.
//!
//! `sync_source` (PR5 catalog-sync integration) is intentionally absent here.

#[cfg(any(feature = "jellyfin", feature = "podcast", feature = "radio"))]
mod common;
pub mod filesystem;
#[cfg(feature = "jellyfin")]
pub mod jellyfin;
#[cfg(feature = "podcast")]
pub mod podcast;
#[cfg(feature = "radio")]
pub mod radio;
pub mod registry;
#[cfg(feature = "radio")]
pub mod somafm;
#[cfg(feature = "subsonic")]
pub mod subsonic;

// Re-export the SPI types so callers only need to depend on `rmpd-source`.
pub use registry::{SOURCE_PLUGINS, SourceFactory, SourcePlugin, create_source};
pub use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult, SyncPolicy};

use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use tracing::warn;

/// Upper bound on songs collected by [`SourceRegistry::expand_on_demand`].
pub const ON_DEMAND_MAX_SONGS: usize = 1000;
/// Upper bound on directories browsed by [`SourceRegistry::expand_on_demand`].
pub const ON_DEMAND_MAX_DIRS: usize = 200;

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

    /// Browse a mount-style path under an [`SyncPolicy::OnDemand`] source by
    /// delegating to [`MusicSource::browse`] (the path after the mount
    /// segment is the source-relative directory; `""` is the source root).
    ///
    /// Returns `None` when no live source owns `path` or the owner is
    /// `Full`-synced (its entries live in the database instead).
    pub async fn browse_on_demand(&self, path: &str) -> Option<SourceResult<Vec<SourceEntry>>> {
        let source = self.owning_source(path)?;
        if source.sync_policy() != SyncPolicy::OnDemand {
            return None;
        }
        let dir = path.split_once('/').map_or("", |(_, rest)| rest);
        Some(source.browse(dir).await)
    }

    /// Resolve a single song under an [`SyncPolicy::OnDemand`] mount via
    /// [`MusicSource::lookup`], so it can be queued without a catalog row.
    ///
    /// The returned song's `path` is forced to `uri` (the mount-style path the
    /// client used) so [`resolve_stream_uri`](Self::resolve_stream_uri) finds
    /// the owning source again at playback time.
    ///
    /// Returns `None` when no live source owns `uri` or the owner is
    /// `Full`-synced; `Some(Ok(None))` when the source does not know the song.
    pub async fn lookup_on_demand(&self, uri: &str) -> Option<SourceResult<Option<Song>>> {
        let source = self.owning_source(uri)?;
        if source.sync_policy() != SyncPolicy::OnDemand {
            return None;
        }
        Some(source.lookup(uri).await.map(|opt| {
            opt.map(|mut song| {
                song.path = camino::Utf8PathBuf::from(uri);
                song
            })
        }))
    }

    /// Expand an [`SyncPolicy::OnDemand`] directory into its songs by walking
    /// [`MusicSource::browse`] depth-first (sub-directories are descended).
    ///
    /// The walk is bounded so a huge remote tree cannot hang or flood the
    /// queue: at most [`ON_DEMAND_MAX_SONGS`] songs are collected and at most
    /// [`ON_DEMAND_MAX_DIRS`] directories are browsed; anything beyond is
    /// silently dropped (a warning is logged). Browse errors on the starting
    /// directory are returned; errors on nested directories are skipped.
    ///
    /// Returns `None` when no live source owns `path` or the owner is
    /// `Full`-synced.
    pub async fn expand_on_demand(&self, path: &str) -> Option<SourceResult<Vec<Song>>> {
        let source = self.owning_source(path)?;
        if source.sync_policy() != SyncPolicy::OnDemand {
            return None;
        }
        Some(expand_dir(source, path).await)
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
/// that source via `Database::with_transaction`: `clear_source` followed by
/// `add_source_song` for each song, all inside one BEGIN/COMMIT. A failure
/// partway through rolls back the whole batch, so the catalog never ends up
/// with the old rows gone and only some new rows inserted. Returns the
/// number of songs inserted.
///
/// This function is `async` because `list_all` does network I/O; the DB work
/// runs on a blocking thread so libsqlite does not stall the Tokio runtime.
pub async fn sync_source(source: &dyn MusicSource, db_path: &str) -> Result<usize, SourceError> {
    if source.sync_policy() == SyncPolicy::OnDemand {
        // Never mirrored: browsing is delegated to `MusicSource::browse`.
        return Ok(0);
    }
    let songs = source.list_all().await?;
    let count = songs.len();
    let token = format!("{}:{}", source.scheme(), source.name());
    let db_path = db_path.to_owned();
    tokio::task::spawn_blocking(move || -> Result<usize, SourceError> {
        let db = rmpd_library::Database::open(&db_path)
            .map_err(|e| SourceError::Protocol(format!("failed to open database: {e}")))?;
        db.with_transaction(|db| {
            db.clear_source(&token)?;
            for song in &songs {
                db.add_source_song(song, &token)?;
            }
            Ok(count)
        })
        .map_err(|e| SourceError::Protocol(format!("sync_source transaction failed: {e}")))
    })
    .await
    .map_err(|e| SourceError::Protocol(format!("sync task panicked: {e}")))?
}

/// Depth-first walk of `start` (a mount-style path) through `source.browse`,
/// bounded by [`ON_DEMAND_MAX_SONGS`] / [`ON_DEMAND_MAX_DIRS`].
async fn expand_dir(source: &dyn MusicSource, start: &str) -> SourceResult<Vec<Song>> {
    let rel = |p: &str| {
        p.split_once('/')
            .map_or(String::new(), |(_, r)| r.to_owned())
    };
    let mut songs: Vec<Song> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Stack of mount-style directory paths still to browse (LIFO; children are
    // pushed reversed so they are visited in listing order).
    let mut stack = vec![start.to_owned()];
    let mut browsed = 0usize;
    seen.insert(start.to_owned());
    while let Some(dir) = stack.pop() {
        if browsed >= ON_DEMAND_MAX_DIRS || songs.len() >= ON_DEMAND_MAX_SONGS {
            warn!(
                "on-demand expansion of {start} truncated ({} songs, {browsed} dirs)",
                songs.len()
            );
            break;
        }
        browsed += 1;
        let entries = match source.browse(&rel(&dir)).await {
            Ok(e) => e,
            Err(e) if dir == start => return Err(e),
            Err(e) => {
                warn!("skipping unbrowsable on-demand directory {dir}: {e}");
                continue;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries {
            match entry {
                SourceEntry::Song(song) => {
                    if songs.len() < ON_DEMAND_MAX_SONGS {
                        songs.push(song);
                    }
                }
                SourceEntry::Dir(d) => {
                    if seen.insert(d.clone()) {
                        subdirs.push(d);
                    }
                }
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(songs)
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
    use async_trait::async_trait;
    use camino::Utf8PathBuf;
    use rmpd_core::song::Song;

    /// Mock `MusicSource` for `sync_source` tests: `list_all` returns a fixed
    /// set of songs.
    struct MockSource {
        songs: Vec<Song>,
    }

    #[async_trait]
    impl MusicSource for MockSource {
        fn scheme(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &str {
            "mock"
        }
        async fn ping(&self) -> SourceResult<()> {
            Ok(())
        }
        async fn browse(&self, _dir: &str) -> SourceResult<Vec<SourceEntry>> {
            Ok(Vec::new())
        }
        async fn list_all(&self) -> SourceResult<Vec<Song>> {
            Ok(self.songs.clone())
        }
        async fn search(&self, _query: &str) -> SourceResult<Vec<Song>> {
            Ok(Vec::new())
        }
        async fn resolve_stream_uri(&self, _song_id: &str) -> SourceResult<String> {
            Err(SourceError::NotFound("mock".to_owned()))
        }
    }

    fn make_song(path: &str) -> Song {
        Song {
            id: 0,
            path: Utf8PathBuf::from(path),
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
            range: None,
            tags: vec![("title".into(), "Test".to_owned())],
        }
    }

    /// A full resync replaces every previously-cached row for the source in
    /// one transaction: a second sync with a smaller song set must leave no
    /// stale rows behind (proves `clear_source` + inserts committed together).
    #[tokio::test]
    async fn sync_source_replaces_stale_rows_atomically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let db_path = db_path.to_str().expect("utf8 path").to_owned();

        let first = MockSource {
            songs: vec![
                make_song("mock/a/one.mp3"),
                make_song("mock/a/two.mp3"),
                make_song("mock/a/three.mp3"),
            ],
        };
        let count = sync_source(&first, &db_path).await.expect("first sync");
        assert_eq!(count, 3);

        let second = MockSource {
            songs: vec![make_song("mock/a/one.mp3")],
        };
        let count = sync_source(&second, &db_path).await.expect("second sync");
        assert_eq!(count, 1);

        // Verify the stale rows from the first sync are gone, not just
        // shadowed: open the DB directly and count rows for this source.
        let db = rmpd_library::Database::open(&db_path).expect("reopen db");
        let remaining = db
            .list_all_songs()
            .expect("list songs")
            .into_iter()
            .filter(|s| s.path.as_str().starts_with("mock/a/"))
            .count();
        assert_eq!(
            remaining, 1,
            "resync must remove rows dropped from the source"
        );
    }

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

    /// On-demand mock named `od`: root has a dir `od/a` and song `od/root`;
    /// `od/a` has song `od/a/one`. `lookup` knows only ids `root` / `one`
    /// and returns songs with a *different* path to prove the override.
    struct OnDemandMock;

    #[async_trait]
    impl MusicSource for OnDemandMock {
        fn scheme(&self) -> &str {
            "od"
        }
        fn name(&self) -> &str {
            "od"
        }
        fn sync_policy(&self) -> SyncPolicy {
            SyncPolicy::OnDemand
        }
        async fn ping(&self) -> SourceResult<()> {
            Ok(())
        }
        async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
            match dir {
                "" => Ok(vec![
                    SourceEntry::Dir("od/a".to_owned()),
                    SourceEntry::Song(make_song("od/root")),
                ]),
                "a" => Ok(vec![SourceEntry::Song(make_song("od/a/one"))]),
                _ => Err(SourceError::NotFound(dir.to_owned())),
            }
        }
        async fn list_all(&self) -> SourceResult<Vec<Song>> {
            Ok(Vec::new())
        }
        async fn search(&self, _query: &str) -> SourceResult<Vec<Song>> {
            Ok(Vec::new())
        }
        async fn resolve_stream_uri(&self, _song_id: &str) -> SourceResult<String> {
            Ok("http://x".to_owned())
        }
        async fn lookup(&self, uri: &str) -> SourceResult<Option<Song>> {
            match uri.rsplit('/').next() {
                Some("root" | "one") => Ok(Some(make_song("elsewhere"))),
                _ => Ok(None),
            }
        }
    }

    fn on_demand_registry() -> SourceRegistry {
        SourceRegistry {
            sources: vec![Box::new(OnDemandMock) as Box<dyn MusicSource>],
        }
    }

    #[tokio::test]
    async fn lookup_on_demand_forces_requested_path() {
        let reg = on_demand_registry();
        let song = reg
            .lookup_on_demand("od/a/one")
            .await
            .expect("owned on-demand")
            .expect("lookup ok")
            .expect("song found");
        assert_eq!(song.path.as_str(), "od/a/one");
        // Unknown id: source answers None; unowned / Full-synced: registry None.
        assert!(matches!(reg.lookup_on_demand("od/a").await, Some(Ok(None))));
        assert!(reg.lookup_on_demand("zzz/x").await.is_none());
        assert!(
            make_registry_with_home()
                .lookup_on_demand("home/x")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn expand_on_demand_walks_subdirectories() {
        let reg = on_demand_registry();
        let songs = reg
            .expand_on_demand("od")
            .await
            .expect("owned")
            .expect("browse ok");
        let paths: Vec<&str> = songs.iter().map(|s| s.path.as_str()).collect();
        // Like MPD's Directory::Walk: a directory's songs, then its children.
        assert_eq!(paths, ["od/root", "od/a/one"]);
        assert!(matches!(
            reg.expand_on_demand("od/missing").await,
            Some(Err(SourceError::NotFound(_)))
        ));
    }
}
