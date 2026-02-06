/// Test harness for testing rmpd-library components directly
///
/// This harness provides a convenient interface for testing the Scanner,
/// Database, and MetadataExtractor in isolation.
use rmpd_core::error::Result;
use rmpd_core::song::Song;
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
    use rmpd_core::song::Song;
    use std::time::Duration;

    fn create_test_song() -> Song {
        Song {
            id: 0,
            path: "/music/test.mp3".into(),
            duration: Some(Duration::from_secs(180)),
            title: Some("Test Song".to_string()),
            artist: Some("Test Artist".to_string()),
            album: Some("Test Album".to_string()),
            album_artist: None,
            track: Some(1),
            disc: None,
            date: Some("2024".to_string()),
            genre: Some("Rock".to_string()),
            composer: None,
            performer: None,
            comment: None,
            musicbrainz_trackid: None,
            musicbrainz_albumid: None,
            musicbrainz_artistid: None,
            musicbrainz_albumartistid: None,
            musicbrainz_releasegroupid: None,
            musicbrainz_releasetrackid: None,
            artist_sort: None,
            album_artist_sort: None,
            original_date: None,
            label: None,
            sample_rate: Some(44100),
            channels: Some(2),
            bits_per_sample: Some(16),
            bitrate: Some(320),
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
        }
    }

    #[test]
    fn test_harness_creation() {
        let harness = RmpdTestHarness::new().unwrap();
        assert!(harness.music_dir.exists());
    }

    #[test]
    fn test_add_and_retrieve_song() {
        let harness = RmpdTestHarness::new().unwrap();
        let song = create_test_song();

        let id = harness.add_song(&song).unwrap();
        assert!(id > 0);

        let retrieved = harness.get_song(id).unwrap().unwrap();
        assert_eq!(retrieved.title, song.title);
        assert_eq!(retrieved.artist, song.artist);
    }

    #[test]
    fn test_list_artists() {
        let harness = RmpdTestHarness::new().unwrap();

        let mut song1 = create_test_song();
        song1.path = "/music/song1.mp3".into();
        song1.artist = Some("Artist A".to_string());
        harness.add_song(&song1).unwrap();

        let mut song2 = create_test_song();
        song2.path = "/music/song2.mp3".into();
        song2.artist = Some("Artist B".to_string());
        harness.add_song(&song2).unwrap();

        let artists = harness.list_artists().unwrap();
        assert_eq!(artists.len(), 2);
        assert!(artists.contains(&"Artist A".to_string()));
        assert!(artists.contains(&"Artist B".to_string()));
    }

    #[test]
    fn test_find_by_artist() {
        let harness = RmpdTestHarness::new().unwrap();

        let mut song = create_test_song();
        song.artist = Some("Target Artist".to_string());
        harness.add_song(&song).unwrap();

        let found = harness.find_by_artist("Target Artist").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artist, Some("Target Artist".to_string()));
    }

    #[test]
    fn test_count_operations() {
        let harness = RmpdTestHarness::new().unwrap();

        let mut song1 = create_test_song();
        song1.path = "/music/song1.mp3".into();
        harness.add_song(&song1).unwrap();

        let mut song2 = create_test_song();
        song2.path = "/music/song2.mp3".into();
        harness.add_song(&song2).unwrap();

        assert_eq!(harness.count_songs().unwrap(), 2);
        assert_eq!(harness.count_artists().unwrap(), 1); // Same artist
    }
}
