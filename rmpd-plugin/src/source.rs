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
}
