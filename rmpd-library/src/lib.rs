// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(clippy::cargo_common_metadata)]

// Music library and database
pub mod artwork;
pub mod cue;
pub mod database;
pub mod embedded_cue;
#[cfg(feature = "fingerprint")]
pub mod fingerprint;
pub mod metadata;
pub mod scanner;
pub mod watcher;

pub use artwork::{
    AlbumArtExtractor, ArtLookup, ArtworkData, ExternalArtwork, NEGATIVE_TTL_SECS, RemoteArtState,
    TRANSIENT_TTL_SECS, classify_remote_entry, find_external_cover, slice_artwork, unix_now,
};
pub use cue::{CueTrack, parse_cue};
pub use database::{Database, DbPool, DirectoryListing, PlaylistInfo, WalkEntry};
#[cfg(feature = "fingerprint")]
pub use fingerprint::Fingerprinter;
pub use metadata::{Artwork, MetadataExtractor};
pub use scanner::{ScanStats, Scanner};
pub use watcher::FilesystemWatcher;
