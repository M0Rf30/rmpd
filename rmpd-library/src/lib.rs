// Music library and database
pub mod database;
pub mod scanner;
pub mod metadata;
pub mod watcher;
pub mod artwork;

pub use database::Database;
pub use scanner::{Scanner, ScanStats};
pub use metadata::MetadataExtractor;
pub use watcher::FilesystemWatcher;
pub use artwork::{AlbumArtExtractor, ArtworkData};
