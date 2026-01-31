#![allow(clippy::cargo_common_metadata)]

// Music library and database
pub mod artwork;
pub mod database;
pub mod metadata;
pub mod scanner;
pub mod watcher;

pub use artwork::{AlbumArtExtractor, ArtworkData};
pub use database::{Database, DirectoryListing, PlaylistInfo};
pub use metadata::MetadataExtractor;
pub use scanner::{ScanStats, Scanner};
pub use watcher::FilesystemWatcher;
