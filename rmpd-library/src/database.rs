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
}
