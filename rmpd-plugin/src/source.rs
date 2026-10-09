// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `MusicSource` SPI — transport-agnostic music-source trait + error types.
//!
//! Lives in `rmpd-plugin` so it can be a dependency-light contract crate
//! (`rmpd-core` + `async-trait` only). Concrete backends live in `rmpd-source`.

use async_trait::async_trait;
use rmpd_core::song::Song;
use std::fmt;

// ─── Error ───────────────────────────────────────────────────────────────────

/// Transport-agnostic source error.
///
/// `Display` and `Debug` implementations MUST NOT echo credentials or secrets.
/// The inner `String` carries an **opaque** message safe to log.
#[derive(Debug)]
pub enum SourceError {
    /// Network unreachable, DNS failure, TLS error, or connection timeout.
    Unreachable(String),
    /// Server rejected credentials (401 / 403).
    Auth(String),
    /// Unknown id or virtual path (404-equivalent).
    NotFound(String),
    /// Malformed server response or unexpected protocol behaviour.
    Protocol(String),
    /// Missing or invalid configuration (URL, credentials, settings).
    Config(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately opaque: the variant tag + a safe summary only.
        // The inner String must already be scrubbed of secrets by the caller.
        match self {
            SourceError::Unreachable(msg) => write!(f, "source unreachable: {msg}"),
            SourceError::Auth(msg) => write!(f, "source auth error: {msg}"),
            SourceError::NotFound(msg) => write!(f, "source not found: {msg}"),
            SourceError::Protocol(msg) => write!(f, "source protocol error: {msg}"),
            SourceError::Config(msg) => write!(f, "source config error: {msg}"),
        }
    }
}

impl std::error::Error for SourceError {}

// ─── Result alias ────────────────────────────────────────────────────────────

pub type SourceResult<T> = Result<T, SourceError>;

// ─── SyncPolicy ──────────────────────────────────────────────────────────────

/// How a source's catalog is exposed to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncPolicy {
    /// `list_all` is mirrored into the local database on `update` (default).
    #[default]
    Full,
    /// Catalog is too large/dynamic to mirror: `list_all` is never called and
    /// browsing under the source's mount delegates to [`MusicSource::browse`].
    OnDemand,
}

// ─── SourceEntry ─────────────────────────────────────────────────────────────

/// One child in a virtual browse listing (one `lsinfo` level).
pub enum SourceEntry {
    /// A playable track; tags + virtual `path` already populated.
    Song(Song),
    /// A virtual subdirectory (full virtual path, e.g. `"subsonic://home/AC%2FDC"`).
    Dir(String),
}

// ─── MusicSource trait ───────────────────────────────────────────────────────

/// Object-safe, `Send + Sync` trait that every music-source backend implements.
///
/// Selection is compile-time (sync const fn-pointer table in `rmpd-source`);
/// the methods here are async because I/O happens when you *call* them, never
/// at registry lookup time.
#[async_trait]
pub trait MusicSource: Send + Sync {
    /// URI scheme this backend owns, e.g. `"subsonic"`, `"file"`.
    fn scheme(&self) -> &str;

    /// Instance name from `[[source]] name =`. Becomes the authority component
    /// of the virtual path: `<scheme>://<name>/...`.
    fn name(&self) -> &str;

    /// Cheap liveness / auth probe. MUST NOT log credentials.
    async fn ping(&self) -> SourceResult<()>;

    /// List immediate children of a virtual directory (`""` = source root).
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>>;

    /// Full catalog enumeration for `update` / sync → DB population.
    /// Each returned `Song` carries MPD tags + its virtual `path`.
    async fn list_all(&self) -> SourceResult<Vec<Song>>;

    /// Server-side search (maps to MPD `find`/`search` base).
    async fn search(&self, query: &str) -> SourceResult<Vec<Song>>;

    /// Map a remote song id to a directly-playable `http(s)://` stream URL,
    /// consumed unchanged by `rmpd_stream::HttpSource` via `decoder.rs`.
    /// Returns `String` (not `url::Url`) so `rmpd-plugin` never needs `url`
    /// or `reqwest`.
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String>;

    /// Fetch raw cover-art bytes for a song id (e.g. Subsonic `getCoverArt`).
    ///
    /// Default: `Ok(None)` — filesystem-backed sources serve embedded art via
    /// the local extractor, so they need not implement this. Remote sources
    /// override it; the caller caches the bytes and infers the MIME type.
    async fn cover_art(&self, _song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        Ok(None)
    }

    /// Resolve a single virtual path / remote URI to a `Song` (tags + path)
    /// without a full catalog sync. Default: `Ok(None)` (unsupported).
    async fn lookup(&self, _uri: &str) -> SourceResult<Option<Song>> {
        Ok(None)
    }

    /// Names of server-side playlists the source exposes. Default: none.
    async fn playlists(&self) -> SourceResult<Vec<String>> {
        Ok(Vec::new())
    }

    /// Songs of the server-side playlist `name`. Default: `NotFound`.
    async fn playlist_items(&self, name: &str) -> SourceResult<Vec<Song>> {
        Err(SourceError::NotFound(format!("playlist: {name}")))
    }

    /// Whether `song_id` is an unbounded live stream (radio, ...): no known
    /// duration, not seekable. Default: `false`.
    fn is_live(&self, _song_id: &str) -> bool {
        false
    }

    /// How the catalog is mirrored into the local database.
    fn sync_policy(&self) -> SyncPolicy {
        SyncPolicy::Full
    }
}
