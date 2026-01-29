use camino::Utf8PathBuf;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use rusqlite::{params, Connection, OptionalExtension};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn init_schema(&self) -> Result<()> {
        // Songs table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS songs (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                directory_id INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                duration REAL,

                -- Core metadata
                title TEXT,
                artist TEXT,
                album TEXT,
                album_artist TEXT,
                track INTEGER,
                disc INTEGER,
                date TEXT,
                genre TEXT,
                composer TEXT,
                performer TEXT,
                comment TEXT,

                -- Audio properties
                sample_rate INTEGER,
                channels INTEGER,
                bits_per_sample INTEGER,
                bitrate INTEGER,

                -- ReplayGain
                replay_gain_track_gain REAL,
                replay_gain_track_peak REAL,
                replay_gain_album_gain REAL,
                replay_gain_album_peak REAL,

                -- Timestamps
                added_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                last_modified INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),

                FOREIGN KEY (directory_id) REFERENCES directories(id)
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Directories table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS directories (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                parent_id INTEGER,
                mtime INTEGER NOT NULL,

                FOREIGN KEY (parent_id) REFERENCES directories(id)
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Artists table (normalized)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Albums table (normalized)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS albums (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                artist_id INTEGER,
                date TEXT,

                UNIQUE(name, artist_id),
                FOREIGN KEY (artist_id) REFERENCES artists(id)
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Playlists table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                mtime INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Playlist items
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS playlist_items (
                id INTEGER PRIMARY KEY,
                playlist_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                song_id INTEGER,
                uri TEXT NOT NULL,

                FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
                FOREIGN KEY (song_id) REFERENCES songs(id) ON DELETE SET NULL
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Stickers (arbitrary key-value metadata)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS stickers (
                id INTEGER PRIMARY KEY,
                uri TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,

                UNIQUE(uri, name)
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Artwork table (album art cache)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS artwork (
                id INTEGER PRIMARY KEY,
                song_path TEXT NOT NULL,
                picture_type TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                data BLOB NOT NULL,
                size INTEGER NOT NULL,
                hash TEXT NOT NULL,

                UNIQUE(song_path, picture_type),
                FOREIGN KEY (song_path) REFERENCES songs(path) ON DELETE CASCADE
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Full-text search using FTS5
        self.conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts USING fts5(
                title, artist, album, album_artist, genre, composer,
                content='songs',
                content_rowid='id'
            )",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Create indexes
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_songs_artist ON songs(artist)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_songs_album ON songs(album)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_songs_album_artist ON songs(album_artist)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_songs_directory ON songs(directory_id)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_directories_parent ON directories(parent_id)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_artwork_path ON artwork(song_path)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_artwork_hash ON artwork(hash)",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Triggers to keep FTS index in sync
        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_insert AFTER INSERT ON songs BEGIN
                INSERT INTO songs_fts(rowid, title, artist, album, album_artist, genre, composer)
                VALUES (new.id, new.title, new.artist, new.album, new.album_artist, new.genre, new.composer);
            END",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_delete AFTER DELETE ON songs BEGIN
                DELETE FROM songs_fts WHERE rowid = old.id;
            END",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_update AFTER UPDATE ON songs BEGIN
                DELETE FROM songs_fts WHERE rowid = old.id;
                INSERT INTO songs_fts(rowid, title, artist, album, album_artist, genre, composer)
                VALUES (new.id, new.title, new.artist, new.album, new.album_artist, new.genre, new.composer);
            END",
            [],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(())
    }

    pub fn add_song(&self, song: &Song) -> Result<u64> {
        // First, ensure directory exists
        let root_path = Utf8PathBuf::from("/");
        let dir_path = song.path.parent().unwrap_or(root_path.as_path());
        let dir_id = self.get_or_create_directory(dir_path)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO songs (
                path, directory_id, mtime, duration,
                title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                sample_rate, channels, bits_per_sample, bitrate,
                replay_gain_track_gain, replay_gain_track_peak,
                replay_gain_album_gain, replay_gain_album_peak
            ) VALUES (
                ?1, ?2, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23
            )",
            params![
                song.path.as_str(),
                dir_id,
                song.last_modified,
                song.duration.map(|d| d.as_secs_f64()),
                song.title,
                song.artist,
                song.album,
                song.album_artist,
                song.track,
                song.disc,
                song.date,
                song.genre,
                song.composer,
                song.performer,
                song.comment,
                song.sample_rate,
                song.channels,
                song.bits_per_sample,
                song.bitrate,
                song.replay_gain_track_gain,
                song.replay_gain_track_peak,
                song.replay_gain_album_gain,
                song.replay_gain_album_peak,
            ],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn get_song(&self, id: u64) -> Result<Option<Song>> {
        self.conn.query_row(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE id = ?1",
            params![id],
            |row| {
                Ok(Song {
                    id: row.get(0)?,
                    path: row.get::<_, String>(1)?.into(),
                    duration: row.get::<_, Option<f64>>(3)?.map(Duration::from_secs_f64),
                    title: row.get(4)?,
                    artist: row.get(5)?,
                    album: row.get(6)?,
                    album_artist: row.get(7)?,
                    track: row.get(8)?,
                    disc: row.get(9)?,
                    date: row.get(10)?,
                    genre: row.get(11)?,
                    composer: row.get(12)?,
                    performer: row.get(13)?,
                    comment: row.get(14)?,
                    sample_rate: row.get(15)?,
                    channels: row.get(16)?,
                    bits_per_sample: row.get(17)?,
                    bitrate: row.get(18)?,
                    replay_gain_track_gain: row.get(19)?,
                    replay_gain_track_peak: row.get(20)?,
                    replay_gain_album_gain: row.get(21)?,
                    replay_gain_album_peak: row.get(22)?,
                    added_at: row.get(23)?,
                    last_modified: row.get(24)?,
                })
            },
        )
        .optional()
        .map_err(|e| RmpdError::Database(e.to_string()))
    }

    pub fn get_song_by_path(&self, path: &str) -> Result<Option<Song>> {
        self.conn.query_row(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE path = ?1",
            params![path],
            |row| {
                Ok(Song {
                    id: row.get(0)?,
                    path: row.get::<_, String>(1)?.into(),
                    duration: row.get::<_, Option<f64>>(3)?.map(Duration::from_secs_f64),
                    title: row.get(4)?,
                    artist: row.get(5)?,
                    album: row.get(6)?,
                    album_artist: row.get(7)?,
                    track: row.get(8)?,
                    disc: row.get(9)?,
                    date: row.get(10)?,
                    genre: row.get(11)?,
                    composer: row.get(12)?,
                    performer: row.get(13)?,
                    comment: row.get(14)?,
                    sample_rate: row.get(15)?,
                    channels: row.get(16)?,
                    bits_per_sample: row.get(17)?,
                    bitrate: row.get(18)?,
                    replay_gain_track_gain: row.get(19)?,
                    replay_gain_track_peak: row.get(20)?,
                    replay_gain_album_gain: row.get(21)?,
                    replay_gain_album_peak: row.get(22)?,
                    added_at: row.get(23)?,
                    last_modified: row.get(24)?,
                })
            },
        )
        .optional()
        .map_err(|e| RmpdError::Database(e.to_string()))
    }

    pub fn count_songs(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))
    }

    pub fn count_artists(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(DISTINCT artist) FROM songs WHERE artist IS NOT NULL", [], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))
    }

    pub fn count_albums(&self) -> Result<u32> {
        self.conn
            .query_row("SELECT COUNT(DISTINCT album) FROM songs WHERE album IS NOT NULL", [], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))
    }

    fn get_or_create_directory(&self, path: &camino::Utf8Path) -> Result<i64> {
        // Check if directory already exists
        if let Some(id) = self.conn.query_row(
            "SELECT id FROM directories WHERE path = ?1",
            params![path.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| RmpdError::Database(e.to_string()))? {
            return Ok(id);
        }

        // Create parent directory if needed
        let parent_id = if let Some(parent) = path.parent() {
            Some(self.get_or_create_directory(parent)?)
        } else {
            None
        };

        // Create this directory
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.conn.execute(
            "INSERT INTO directories (path, parent_id, mtime) VALUES (?1, ?2, ?3)",
            params![path.as_str(), parent_id, now],
        )
        .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn search_songs(&self, query: &str) -> Result<Vec<Song>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.path, s.mtime, s.duration,
                    s.title, s.artist, s.album, s.album_artist, s.track, s.disc, s.date, s.genre, s.composer, s.performer, s.comment,
                    s.sample_rate, s.channels, s.bits_per_sample, s.bitrate,
                    s.replay_gain_track_gain, s.replay_gain_track_peak,
                    s.replay_gain_album_gain, s.replay_gain_album_peak,
                    s.added_at, s.last_modified
             FROM songs s
             JOIN songs_fts ON songs_fts.rowid = s.id
             WHERE songs_fts MATCH ?1
             ORDER BY rank"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let songs = stmt.query_map(params![query], |row| {
            Ok(Song {
                id: row.get(0)?,
                path: row.get::<_, String>(1)?.into(),
                duration: row.get::<_, Option<f64>>(3)?.map(Duration::from_secs_f64),
                title: row.get(4)?,
                artist: row.get(5)?,
                album: row.get(6)?,
                album_artist: row.get(7)?,
                track: row.get(8)?,
                disc: row.get(9)?,
                date: row.get(10)?,
                genre: row.get(11)?,
                composer: row.get(12)?,
                performer: row.get(13)?,
                comment: row.get(14)?,
                sample_rate: row.get(15)?,
                channels: row.get(16)?,
                bits_per_sample: row.get(17)?,
                bitrate: row.get(18)?,
                replay_gain_track_gain: row.get(19)?,
                replay_gain_track_peak: row.get(20)?,
                replay_gain_album_gain: row.get(21)?,
                replay_gain_album_peak: row.get(22)?,
                added_at: row.get(23)?,
                last_modified: row.get(24)?,
            })
        })
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(songs)
    }

    pub fn list_artists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT artist FROM songs WHERE artist IS NOT NULL ORDER BY artist COLLATE NOCASE"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let artists = stmt.query_map([], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(artists)
    }

    pub fn list_albums(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT album FROM songs WHERE album IS NOT NULL ORDER BY album COLLATE NOCASE"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let albums = stmt.query_map([], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(albums)
    }

    pub fn list_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT genre FROM songs WHERE genre IS NOT NULL ORDER BY genre COLLATE NOCASE"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let genres = stmt.query_map([], |row| row.get(0))
            .map_err(|e| RmpdError::Database(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(genres)
    }

    pub fn find_songs(&self, tag: &str, value: &str) -> Result<Vec<Song>> {
        let query = match tag.to_lowercase().as_str() {
            "artist" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE artist = ?1 ORDER BY album, track",
            "album" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE album = ?1 ORDER BY track",
            "genre" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE genre = ?1 ORDER BY artist, album, track",
            _ => return Err(RmpdError::Library(format!("Unsupported tag: {}", tag))),
        };

        let mut stmt = self.conn.prepare(query)
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        let songs = stmt.query_map(params![value], |row| {
            Ok(Song {
                id: row.get(0)?,
                path: row.get::<_, String>(1)?.into(),
                duration: row.get::<_, Option<f64>>(3)?.map(Duration::from_secs_f64),
                title: row.get(4)?,
                artist: row.get(5)?,
                album: row.get(6)?,
                album_artist: row.get(7)?,
                track: row.get(8)?,
                disc: row.get(9)?,
                date: row.get(10)?,
                genre: row.get(11)?,
                composer: row.get(12)?,
                performer: row.get(13)?,
                comment: row.get(14)?,
                sample_rate: row.get(15)?,
                channels: row.get(16)?,
                bits_per_sample: row.get(17)?,
                bitrate: row.get(18)?,
                replay_gain_track_gain: row.get(19)?,
                replay_gain_track_peak: row.get(20)?,
                replay_gain_album_gain: row.get(21)?,
                replay_gain_album_peak: row.get(22)?,
                added_at: row.get(23)?,
                last_modified: row.get(24)?,
            })
        })
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(songs)
    }

    pub fn list_all_songs(&self) -> Result<Vec<Song>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs
             ORDER BY path"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let songs = stmt.query_map([], |row| {
            Ok(Song {
                id: row.get(0)?,
                path: row.get::<_, String>(1)?.into(),
                duration: row.get::<_, Option<f64>>(3)?.map(Duration::from_secs_f64),
                title: row.get(4)?,
                artist: row.get(5)?,
                album: row.get(6)?,
                album_artist: row.get(7)?,
                track: row.get(8)?,
                disc: row.get(9)?,
                date: row.get(10)?,
                genre: row.get(11)?,
                composer: row.get(12)?,
                performer: row.get(13)?,
                comment: row.get(14)?,
                sample_rate: row.get(15)?,
                channels: row.get(16)?,
                bits_per_sample: row.get(17)?,
                bitrate: row.get(18)?,
                replay_gain_track_gain: row.get(19)?,
                replay_gain_track_peak: row.get(20)?,
                replay_gain_album_gain: row.get(21)?,
                replay_gain_album_peak: row.get(22)?,
                added_at: row.get(23)?,
                last_modified: row.get(24)?,
            })
        })
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(songs)
    }

    pub fn delete_song_by_path(&self, path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM songs WHERE path = ?1",
            params![path],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;
        Ok(())
    }

    // Artwork methods
    pub fn get_artwork(&self, path: &str, picture_type: &str) -> Result<Option<Vec<u8>>> {
        self.conn.query_row(
            "SELECT data FROM artwork WHERE song_path = ?1 AND picture_type = ?2",
            params![path, picture_type],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| RmpdError::Database(e.to_string()))
    }

    pub fn store_artwork(
        &self,
        path: &str,
        picture_type: &str,
        mime_type: &str,
        data: &[u8],
        hash: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO artwork (song_path, picture_type, mime_type, data, size, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![path, picture_type, mime_type, data, data.len() as i64, hash],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn has_artwork(&self, path: &str, picture_type: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artwork WHERE song_path = ?1 AND picture_type = ?2",
            params![path, picture_type],
            |row| row.get(0),
        ).map_err(|e| RmpdError::Database(e.to_string()))?;
        Ok(count > 0)
    }

    /// List directory contents (songs + subdirectories)
    pub fn list_directory(&self, path: &str) -> Result<DirectoryListing> {
        // First, find the directory
        let dir_id = if path.is_empty() {
            // Root directory - get all top-level items
            None
        } else {
            // Find directory by path
            let id: Option<i64> = self.conn.query_row(
                "SELECT id FROM directories WHERE path = ?1",
                params![path],
                |row| row.get(0),
            ).optional()
            .map_err(|e| RmpdError::Database(e.to_string()))?;
            id
        };

        // Get subdirectories
        let mut directories = Vec::new();
        if let Some(id) = dir_id {
            let mut stmt = self.conn.prepare("SELECT path FROM directories WHERE parent_id = ?1 ORDER BY path")
                .map_err(|e| RmpdError::Database(e.to_string()))?;
            let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0))
                .map_err(|e| RmpdError::Database(e.to_string()))?;
            for row in rows {
                directories.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
            }
        } else {
            let mut stmt = self.conn.prepare("SELECT path FROM directories WHERE parent_id IS NULL ORDER BY path")
                .map_err(|e| RmpdError::Database(e.to_string()))?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| RmpdError::Database(e.to_string()))?;
            for row in rows {
                directories.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
            }
        }

        // Get songs in this directory
        let mut songs = Vec::new();
        {
            let query = "SELECT id, path, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
                FROM songs WHERE directory_id = ?1 ORDER BY path";

            let mut stmt = self.conn.prepare(query)
                .map_err(|e| RmpdError::Database(e.to_string()))?;

            let song_rows = stmt.query_map(params![dir_id.unwrap_or(0)], |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
                    path: Utf8PathBuf::from(row.get::<_, String>(1)?),
                    duration: row.get::<_, Option<f64>>(2)?.map(Duration::from_secs_f64),
                    title: row.get(3).ok(),
                    artist: row.get(4).ok(),
                    album: row.get(5).ok(),
                    album_artist: row.get(6).ok(),
                    track: row.get(7).ok(),
                    disc: row.get(8).ok(),
                    date: row.get(9).ok(),
                    genre: row.get(10).ok(),
                    composer: row.get(11).ok(),
                    performer: row.get(12).ok(),
                    comment: row.get(13).ok(),
                    sample_rate: row.get(14).ok(),
                    channels: row.get(15).ok(),
                    bits_per_sample: row.get(16).ok(),
                    bitrate: row.get(17).ok(),
                    replay_gain_track_gain: row.get(18).ok(),
                    replay_gain_track_peak: row.get(19).ok(),
                    replay_gain_album_gain: row.get(20).ok(),
                    replay_gain_album_peak: row.get(21).ok(),
                    added_at: row.get(22)?,
                    last_modified: row.get(23)?,
                })
            }).map_err(|e| RmpdError::Database(e.to_string()))?;

            for row in song_rows {
                songs.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
            }
        }

        Ok(DirectoryListing { directories, songs })
    }

    /// List all songs under a directory recursively
    pub fn list_directory_recursive(&self, path: &str) -> Result<Vec<Song>> {
        let query = "SELECT id, path, duration,
                title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                sample_rate, channels, bits_per_sample, bitrate,
                replay_gain_track_gain, replay_gain_track_peak,
                replay_gain_album_gain, replay_gain_album_peak,
                added_at, last_modified
            FROM songs WHERE path LIKE ?1 || '%' ORDER BY path";

        let mut stmt = self.conn.prepare(query)
            .map_err(|e| RmpdError::Database(e.to_string()))?;

        let search_path = if path.is_empty() { "%" } else { path };

        let song_rows = stmt.query_map(params![search_path], |row| {
            Ok(Song {
                id: row.get::<_, i64>(0)? as u64,
                path: Utf8PathBuf::from(row.get::<_, String>(1)?),
                duration: row.get::<_, Option<f64>>(2)?.map(Duration::from_secs_f64),
                title: row.get(3).ok(),
                artist: row.get(4).ok(),
                album: row.get(5).ok(),
                album_artist: row.get(6).ok(),
                track: row.get(7).ok(),
                disc: row.get(8).ok(),
                date: row.get(9).ok(),
                genre: row.get(10).ok(),
                composer: row.get(11).ok(),
                performer: row.get(12).ok(),
                comment: row.get(13).ok(),
                sample_rate: row.get(14).ok(),
                channels: row.get(15).ok(),
                bits_per_sample: row.get(16).ok(),
                bitrate: row.get(17).ok(),
                replay_gain_track_gain: row.get(18).ok(),
                replay_gain_track_peak: row.get(19).ok(),
                replay_gain_album_gain: row.get(20).ok(),
                replay_gain_album_peak: row.get(21).ok(),
                added_at: row.get(22)?,
                last_modified: row.get(23)?,
            })
        }).map_err(|e| RmpdError::Database(e.to_string()))?;

        let mut songs = Vec::new();
        for row in song_rows {
            songs.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
        }

        Ok(songs)
    }

    /// Save current queue as a playlist
    pub fn save_playlist(&self, name: &str, songs: &[Song]) -> Result<()> {
        // Create or replace playlist
        self.conn.execute(
            "INSERT OR REPLACE INTO playlists (name, mtime) VALUES (?1, strftime('%s', 'now'))",
            params![name],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Clear existing items
        self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Add songs
        for (position, song) in songs.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO playlist_items (playlist_id, position, song_id, uri) VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, position as i64, song.id as i64, song.path.as_str()],
            ).map_err(|e| RmpdError::Database(e.to_string()))?;
        }

        Ok(())
    }

    /// Load playlist and return songs
    pub fn load_playlist(&self, name: &str) -> Result<Vec<Song>> {
        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {}", name)))?;

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.path, s.duration,
                    s.title, s.artist, s.album, s.album_artist, s.track, s.disc, s.date, s.genre,
                    s.composer, s.performer, s.comment,
                    s.sample_rate, s.channels, s.bits_per_sample, s.bitrate,
                    s.replay_gain_track_gain, s.replay_gain_track_peak,
                    s.replay_gain_album_gain, s.replay_gain_album_peak,
                    s.added_at, s.last_modified
             FROM playlist_items pi
             JOIN songs s ON pi.song_id = s.id
             WHERE pi.playlist_id = ?1
             ORDER BY pi.position"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let song_rows = stmt.query_map(params![playlist_id], |row| {
            Ok(Song {
                id: row.get::<_, i64>(0)? as u64,
                path: Utf8PathBuf::from(row.get::<_, String>(1)?),
                duration: row.get::<_, Option<f64>>(2)?.map(Duration::from_secs_f64),
                title: row.get(3).ok(),
                artist: row.get(4).ok(),
                album: row.get(5).ok(),
                album_artist: row.get(6).ok(),
                track: row.get(7).ok(),
                disc: row.get(8).ok(),
                date: row.get(9).ok(),
                genre: row.get(10).ok(),
                composer: row.get(11).ok(),
                performer: row.get(12).ok(),
                comment: row.get(13).ok(),
                sample_rate: row.get(14).ok(),
                channels: row.get(15).ok(),
                bits_per_sample: row.get(16).ok(),
                bitrate: row.get(17).ok(),
                replay_gain_track_gain: row.get(18).ok(),
                replay_gain_track_peak: row.get(19).ok(),
                replay_gain_album_gain: row.get(20).ok(),
                replay_gain_album_peak: row.get(21).ok(),
                added_at: row.get(22)?,
                last_modified: row.get(23)?,
            })
        }).map_err(|e| RmpdError::Database(e.to_string()))?;

        let mut songs = Vec::new();
        for row in song_rows {
            songs.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
        }

        Ok(songs)
    }

    /// List all playlists
    pub fn list_playlists(&self) -> Result<Vec<PlaylistInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name, p.mtime, COUNT(pi.id) as song_count
             FROM playlists p
             LEFT JOIN playlist_items pi ON p.id = pi.playlist_id
             GROUP BY p.id
             ORDER BY p.name"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let playlist_rows = stmt.query_map([], |row| {
            Ok(PlaylistInfo {
                name: row.get(0)?,
                last_modified: row.get(1)?,
                song_count: row.get(2)?,
            })
        }).map_err(|e| RmpdError::Database(e.to_string()))?;

        let mut playlists = Vec::new();
        for row in playlist_rows {
            playlists.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
        }

        Ok(playlists)
    }

    /// Get songs in a playlist
    pub fn get_playlist_songs(&self, name: &str) -> Result<Vec<Song>> {
        self.load_playlist(name)
    }

    /// Delete a playlist
    pub fn delete_playlist(&self, name: &str) -> Result<()> {
        let affected = self.conn.execute(
            "DELETE FROM playlists WHERE name = ?1",
            params![name],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {}", name)));
        }

        Ok(())
    }

    /// Rename a playlist
    pub fn rename_playlist(&self, from: &str, to: &str) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE playlists SET name = ?1, mtime = strftime('%s', 'now') WHERE name = ?2",
            params![to, from],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {}", from)));
        }

        Ok(())
    }

    /// Add a song to a playlist
    pub fn playlist_add(&self, name: &str, uri: &str) -> Result<()> {
        // Get playlist ID
        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {}", name)))?;

        // Get song by URI
        let song = self.get_song_by_path(uri)?
            .ok_or_else(|| RmpdError::Library(format!("Song not found: {}", uri)))?;

        // Get next position
        let next_pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Add song
        self.conn.execute(
            "INSERT INTO playlist_items (playlist_id, position, song_id, uri) VALUES (?1, ?2, ?3, ?4)",
            params![playlist_id, next_pos, song.id as i64, uri],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Update mtime
        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(())
    }

    /// Clear all songs from a playlist
    pub fn playlist_clear(&self, name: &str) -> Result<()> {
        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {}", name)))?;

        self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Update mtime
        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(())
    }

    /// Delete a song from a playlist by position
    pub fn playlist_delete_pos(&self, name: &str, position: u32) -> Result<()> {
        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {}", name)))?;

        let affected = self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, position],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        if affected == 0 {
            return Err(RmpdError::Library(format!("Position not found: {}", position)));
        }

        // Reindex positions
        self.conn.execute(
            "UPDATE playlist_items SET position = position - 1
             WHERE playlist_id = ?1 AND position > ?2",
            params![playlist_id, position],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Update mtime
        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(())
    }

    /// Move a song within a playlist
    pub fn playlist_move(&self, name: &str, from: u32, to: u32) -> Result<()> {
        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {}", name)))?;

        if from == to {
            return Ok(());
        }

        // This is a bit complex - we need to:
        // 1. Get the item at 'from'
        // 2. Delete it
        // 3. Shift positions
        // 4. Insert at 'to'

        let item_id: i64 = self.conn.query_row(
            "SELECT id FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, from],
            |row| row.get(0),
        ).optional()
        .map_err(|e| RmpdError::Database(e.to_string()))?
        .ok_or_else(|| RmpdError::Library(format!("Position not found: {}", from)))?;

        // Move logic similar to queue
        if from < to {
            // Moving down: shift items between from+1 and to down by 1
            self.conn.execute(
                "UPDATE playlist_items SET position = position - 1
                 WHERE playlist_id = ?1 AND position > ?2 AND position <= ?3",
                params![playlist_id, from, to],
            ).map_err(|e| RmpdError::Database(e.to_string()))?;
        } else {
            // Moving up: shift items between to and from-1 up by 1
            self.conn.execute(
                "UPDATE playlist_items SET position = position + 1
                 WHERE playlist_id = ?1 AND position >= ?2 AND position < ?3",
                params![playlist_id, to, from],
            ).map_err(|e| RmpdError::Database(e.to_string()))?;
        }

        // Set the item's new position
        self.conn.execute(
            "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
            params![to, item_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        // Update mtime
        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        Ok(())
    }

    // Sticker methods (arbitrary key-value metadata for songs/directories)

    /// Get a sticker value by URI and name
    pub fn get_sticker(&self, uri: &str, name: &str) -> Result<Option<String>> {
        self.conn.query_row(
            "SELECT value FROM stickers WHERE uri = ?1 AND name = ?2",
            params![uri, name],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| RmpdError::Database(e.to_string()))
    }

    /// Set a sticker value
    pub fn set_sticker(&self, uri: &str, name: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stickers (uri, name, value) VALUES (?1, ?2, ?3)",
            params![uri, name, value],
        ).map_err(|e| RmpdError::Database(e.to_string()))?;
        Ok(())
    }

    /// Delete sticker(s) for a URI
    /// If name is Some, delete only that sticker
    /// If name is None, delete all stickers for the URI
    pub fn delete_sticker(&self, uri: &str, name: Option<&str>) -> Result<()> {
        if let Some(sticker_name) = name {
            self.conn.execute(
                "DELETE FROM stickers WHERE uri = ?1 AND name = ?2",
                params![uri, sticker_name],
            ).map_err(|e| RmpdError::Database(e.to_string()))?;
        } else {
            self.conn.execute(
                "DELETE FROM stickers WHERE uri = ?1",
                params![uri],
            ).map_err(|e| RmpdError::Database(e.to_string()))?;
        }
        Ok(())
    }

    /// List all stickers for a URI
    pub fn list_stickers(&self, uri: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, value FROM stickers WHERE uri = ?1 ORDER BY name"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let sticker_rows = stmt.query_map(params![uri], |row| {
            Ok((row.get(0)?, row.get(1)?))
        }).map_err(|e| RmpdError::Database(e.to_string()))?;

        let mut stickers = Vec::new();
        for row in sticker_rows {
            stickers.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
        }

        Ok(stickers)
    }

    /// Find all URIs that have a sticker with the given name
    /// Returns (uri, value) pairs
    pub fn find_stickers(&self, uri: &str, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT uri, value FROM stickers WHERE uri LIKE ?1 AND name = ?2 ORDER BY uri"
        ).map_err(|e| RmpdError::Database(e.to_string()))?;

        let search_pattern = if uri.is_empty() {
            "%".to_string()
        } else {
            format!("{}%", uri)
        };

        let sticker_rows = stmt.query_map(params![search_pattern, name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        }).map_err(|e| RmpdError::Database(e.to_string()))?;

        let mut results = Vec::new();
        for row in sticker_rows {
            results.push(row.map_err(|e| RmpdError::Database(e.to_string()))?);
        }

        Ok(results)
    }
}

/// Directory listing result
pub struct DirectoryListing {
    pub directories: Vec<String>,
    pub songs: Vec<Song>,
}

/// Playlist information
pub struct PlaylistInfo {
    pub name: String,
    pub last_modified: i64,
    pub song_count: u32,
}
