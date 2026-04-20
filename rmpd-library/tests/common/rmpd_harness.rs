/// Test harness for testing rmpd-library components directly
///
/// This harness provides a convenient interface for testing the Scanner,
/// Database, and MetadataExtractor in isolation.
use rmpd_core::error::Result;
use rmpd_core::song::{Song, intern_tag_key};
use rmpd_library::database::Database;
use rmpd_library::metadata::{Artwork, MetadataExtractor};
use std::path::PathBuf;
use tempfile::TempDir;

pub struct RmpdTestHarness {
    _temp_dir: TempDir,
    pub music_dir: PathBuf,
    pub database: Database,
}

impl RmpdTestHarness {
    /// Create a new test harness with temporary directories
    pub fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let music_dir = temp_dir.path().join("music");
        std::fs::create_dir(&music_dir)?;

        let db_path = temp_dir.path().join("test.db");
        let database = Database::open(db_path.to_str().unwrap())?;

        Ok(Self {
            _temp_dir: temp_dir,
            music_dir,
            database,
        })
    }

    /// Extract metadata from a file using MetadataExtractor
    pub fn extract_metadata(&self, file_path: &str) -> Result<Song> {
        let path_buf: camino::Utf8PathBuf = file_path.into();
        MetadataExtractor::extract_from_file(&path_buf)
    }

    /// Add a song to the database
    pub fn add_song(&self, song: &Song) -> Result<u64> {
        self.database.add_song(song)
    }

    /// List all artists in the database
    pub fn list_artists(&self) -> Result<Vec<String>> {
        self.database.list_artists()
    }

    /// List all albums in the database
    pub fn list_albums(&self) -> Result<Vec<String>> {
        self.database.list_albums()
    }

    /// Find songs by artist
    pub fn find_by_artist(&self, artist: &str) -> Result<Vec<Song>> {
        self.database.find_songs("artist", artist)
    }

    /// Find songs by album
    pub fn find_by_album(&self, album: &str) -> Result<Vec<Song>> {
        self.database.find_songs("album", album)
    }

    /// Search songs using FTS
    pub fn search(&self, query: &str) -> Result<Vec<Song>> {
        self.database.search_songs(query)
    }

    /// Count total songs
    pub fn count_songs(&self) -> Result<u32> {
        self.database.count_songs()
    }

    /// Count total artists
    pub fn count_artists(&self) -> Result<u32> {
        self.database.count_artists()
    }

    /// Count total albums
    pub fn count_albums(&self) -> Result<u32> {
        self.database.count_albums()
    }

    /// Get a song by ID
    pub fn get_song(&self, id: u64) -> Result<Option<Song>> {
        self.database.get_song(id)
    }

    /// Get a song by path
    pub fn get_song_by_path(&self, path: &str) -> Result<Option<Song>> {
        self.database.get_song_by_path(path)
    }

    /// Check if artwork exists for a song
    pub fn has_artwork(&self, path: &str, picture_type: &str) -> Result<bool> {
        self.database.has_artwork(path, picture_type)
    }

    /// Get artwork data
    pub fn get_artwork(&self, path: &str, picture_type: &str) -> Result<Option<Vec<u8>>> {
        self.database.get_artwork(path, picture_type)
    }

    /// Extract artwork from a file
    pub fn extract_artwork(&self, file_path: &str) -> Result<Vec<Artwork>> {
        let path_buf: camino::Utf8PathBuf = file_path.into();
        MetadataExtractor::extract_artwork_from_file(&path_buf)
    }

    /// Store artwork in the database
    pub fn store_artwork(&self, path: &str, artwork: &Artwork) -> Result<()> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&artwork.data);
        let hash = format!("{:x}", hasher.finalize());

        self.database.store_artwork(
            path,
            &artwork.picture_type,
            &artwork.mime_type,
            &artwork.data,
            &hash,
        )
    }
}

impl Default for RmpdTestHarness {
    fn default() -> Self {
        Self::new().expect("Failed to create test harness")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::test_utils::make_test_song;

    #[test]
    fn test_harness_creation() {
        let harness = RmpdTestHarness::new().unwrap();
        assert!(harness.music_dir.exists());
    }

    #[test]
    fn test_add_and_retrieve_song() {
        let harness = RmpdTestHarness::new().unwrap();
        let song = make_test_song("/music/test.mp3", 0);

        let id = harness.add_song(&song).unwrap();
        assert!(id > 0);

        let retrieved = harness.get_song(id).unwrap().unwrap();
        assert_eq!(retrieved.tag("title"), song.tag("title"));
        assert_eq!(retrieved.tag("artist"), song.tag("artist"));
    }

    #[test]
    fn test_list_artists() {
        let harness = RmpdTestHarness::new().unwrap();

        let mut song1 = make_test_song("/music/song1.mp3", 0);
        song1.tags.retain(|(k, _)| k.as_ref() != "artist");
        song1
            .tags
            .push((intern_tag_key("artist"), "Artist A".to_string()));
        harness.add_song(&song1).unwrap();

        let mut song2 = make_test_song("/music/song2.mp3", 1);
        song2.tags.retain(|(k, _)| k.as_ref() != "artist");
        song2
            .tags
            .push((intern_tag_key("artist"), "Artist B".to_string()));
        harness.add_song(&song2).unwrap();

        let artists = harness.list_artists().unwrap();
        assert_eq!(artists.len(), 2);
        assert!(artists.contains(&"Artist A".to_string()));
        assert!(artists.contains(&"Artist B".to_string()));
    }

    #[test]
    fn test_find_by_artist() {
        let harness = RmpdTestHarness::new().unwrap();

        let mut song = make_test_song("/music/test.mp3", 0);
        song.tags.retain(|(k, _)| k.as_ref() != "artist");
        song.tags
            .push((intern_tag_key("artist"), "Target Artist".to_string()));
        harness.add_song(&song).unwrap();

        let found = harness.find_by_artist("Target Artist").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].tag("artist"), Some("Target Artist"));
    }

    #[test]
    fn test_count_operations() {
        let harness = RmpdTestHarness::new().unwrap();

        let song1 = make_test_song("/music/song1.mp3", 0);
        harness.add_song(&song1).unwrap();

        let song2 = make_test_song("/music/song2.mp3", 1);
        harness.add_song(&song2).unwrap();

        assert_eq!(harness.count_songs().unwrap(), 2);
        assert_eq!(harness.count_artists().unwrap(), 1); // Same artist
    }
}
