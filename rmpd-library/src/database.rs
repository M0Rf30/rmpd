use camino::Utf8PathBuf;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use rusqlite::{params, Connection, OptionalExtension};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn init_schema(&self) -> Result<()> {
        // Songs table
        self.conn
            .execute(
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
            )
            ?;

        // Directories table
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS directories (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                parent_id INTEGER,
                mtime INTEGER NOT NULL,

                FOREIGN KEY (parent_id) REFERENCES directories(id)
            )",
                [],
            )
            ?;

        // Artists table (normalized)
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE
            )",
                [],
            )
            ?;

        // Albums table (normalized)
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS albums (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                artist_id INTEGER,
                date TEXT,

                UNIQUE(name, artist_id),
                FOREIGN KEY (artist_id) REFERENCES artists(id)
            )",
                [],
            )
            ?;

        // Playlists table
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                mtime INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            )",
                [],
            )
            ?;

        // Playlist items
        self.conn
            .execute(
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
            )
            ?;

        // Stickers (arbitrary key-value metadata)
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS stickers (
                id INTEGER PRIMARY KEY,
                uri TEXT NOT NULL,
                name TEXT NOT NULL,
                value TEXT NOT NULL,

                UNIQUE(uri, name)
            )",
                [],
            )
            ?;

        // Artwork table (album art cache)
        self.conn
            .execute(
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
            )
            ?;

        // Full-text search using FTS5
        self.conn
            .execute(
                "CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts USING fts5(
                title, artist, album, album_artist, genre, composer,
                content='songs',
                content_rowid='id'
            )",
                [],
            )
            ?;

        // Create indexes
        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_songs_artist ON songs(artist)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_songs_album ON songs(album)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_songs_album_artist ON songs(album_artist)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_songs_directory ON songs(directory_id)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_directories_parent ON directories(parent_id)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_artwork_path ON artwork(song_path)",
                [],
            )
            ?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_artwork_hash ON artwork(hash)",
                [],
            )
            ?;

        // Run migrations for new fields
        self.migrate_add_musicbrainz_fields()?;

        // Triggers to keep FTS index in sync
        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_insert AFTER INSERT ON songs BEGIN
                INSERT INTO songs_fts(rowid, title, artist, album, album_artist, genre, composer)
                VALUES (new.id, new.title, new.artist, new.album, new.album_artist, new.genre, new.composer);
            END",
            [],
        )?;

        self.conn
            .execute(
                "CREATE TRIGGER IF NOT EXISTS songs_fts_delete AFTER DELETE ON songs BEGIN
                DELETE FROM songs_fts WHERE rowid = old.id;
            END",
                [],
            )
            ?;

        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_update AFTER UPDATE ON songs BEGIN
                DELETE FROM songs_fts WHERE rowid = old.id;
                INSERT INTO songs_fts(rowid, title, artist, album, album_artist, genre, composer)
                VALUES (new.id, new.title, new.artist, new.album, new.album_artist, new.genre, new.composer);
            END",
            [],
        )?;

        Ok(())
    }

    fn migrate_add_musicbrainz_fields(&self) -> Result<()> {
        // Check if columns already exist
        let column_exists: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('songs') WHERE name = 'musicbrainz_trackid'",
            [],
            |row| row.get(0),
        )?;

        if column_exists {
            return Ok(());
        }

        // Add new columns
        self.conn
            .execute_batch(
                "
            ALTER TABLE songs ADD COLUMN musicbrainz_trackid TEXT;
            ALTER TABLE songs ADD COLUMN musicbrainz_albumid TEXT;
            ALTER TABLE songs ADD COLUMN musicbrainz_artistid TEXT;
            ALTER TABLE songs ADD COLUMN musicbrainz_albumartistid TEXT;
            ALTER TABLE songs ADD COLUMN musicbrainz_releasegroupid TEXT;
            ALTER TABLE songs ADD COLUMN musicbrainz_releasetrackid TEXT;
            ALTER TABLE songs ADD COLUMN artist_sort TEXT;
            ALTER TABLE songs ADD COLUMN album_artist_sort TEXT;
            ALTER TABLE songs ADD COLUMN original_date TEXT;
            ALTER TABLE songs ADD COLUMN label TEXT;

            CREATE INDEX IF NOT EXISTS idx_songs_musicbrainz_trackid ON songs(musicbrainz_trackid);
            CREATE INDEX IF NOT EXISTS idx_songs_musicbrainz_albumid ON songs(musicbrainz_albumid);
        ",
            )
            ?;

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
                musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                artist_sort, album_artist_sort, original_date, label,
                sample_rate, channels, bits_per_sample, bitrate,
                replay_gain_track_gain, replay_gain_track_peak,
                replay_gain_album_gain, replay_gain_album_peak
            ) VALUES (
                ?1, ?2, ?3, ?4,
                ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24, ?25,
                ?26, ?27, ?28, ?29,
                ?30, ?31, ?32, ?33
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
                song.musicbrainz_trackid,
                song.musicbrainz_albumid,
                song.musicbrainz_artistid,
                song.musicbrainz_albumartistid,
                song.musicbrainz_releasegroupid,
                song.musicbrainz_releasetrackid,
                song.artist_sort,
                song.album_artist_sort,
                song.original_date,
                song.label,
                song.sample_rate,
                song.channels,
                song.bits_per_sample,
                song.bitrate,
                song.replay_gain_track_gain,
                song.replay_gain_track_peak,
                song.replay_gain_album_gain,
                song.replay_gain_album_peak,
            ],
        )?;

        Ok(self.conn.last_insert_rowid() as u64)
    }

    pub fn get_song(&self, id: u64) -> Result<Option<Song>> {
        Ok(self.conn.query_row(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE id = ?1",
            params![id as i64],
            |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            },
        )
        .optional()?)
    }

    pub fn get_song_by_path(&self, path: &str) -> Result<Option<Song>> {
        Ok(self.conn.query_row(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE path = ?1",
            params![path],
            |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            },
        )
        .optional()?)
    }

    pub fn count_songs(&self) -> Result<u32> {
        Ok(self.conn
            .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))?)
    }

    pub fn count_artists(&self) -> Result<u32> {
        Ok(self.conn
            .query_row(
                "SELECT COUNT(DISTINCT artist) FROM songs WHERE artist IS NOT NULL",
                [],
                |row| row.get(0),
            )?)
    }

    pub fn count_albums(&self) -> Result<u32> {
        Ok(self.conn
            .query_row(
                "SELECT COUNT(DISTINCT album) FROM songs WHERE album IS NOT NULL",
                [],
                |row| row.get(0),
            )?)
    }

    pub fn get_db_playtime(&self) -> Result<u64> {
        Ok(self.conn
            .query_row("SELECT COALESCE(SUM(duration), 0) FROM songs", [], |row| {
                let duration: f64 = row.get(0)?;
                Ok(duration as u64)
            })?)
    }

    pub fn get_db_update(&self) -> Result<i64> {
        Ok(self.conn
            .query_row("SELECT COALESCE(MAX(added_at), 0) FROM songs", [], |row| {
                row.get(0)
            })?)
    }

    fn get_or_create_directory(&self, path: &camino::Utf8Path) -> Result<i64> {
        // Check if directory already exists
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM directories WHERE path = ?1",
                params![path.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            ?
        {
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
            .unwrap_or_else(|_| {
                tracing::warn!("System time before UNIX_EPOCH, using 0");
                Duration::ZERO
            })
            .as_secs() as i64;

        self.conn
            .execute(
                "INSERT INTO directories (path, parent_id, mtime) VALUES (?1, ?2, ?3)",
                params![path.as_str(), parent_id, now],
            )
            ?;

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
        )?;

        let songs = stmt
            .query_map(params![query], |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            })
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(songs)
    }

    pub fn list_artists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT artist FROM songs WHERE artist IS NOT NULL ORDER BY artist COLLATE NOCASE"
        )?;

        let artists = stmt
            .query_map([], |row| row.get(0))
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(artists)
    }

    pub fn list_albums(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT album FROM songs WHERE album IS NOT NULL ORDER BY album COLLATE NOCASE"
        )?;

        let albums = stmt
            .query_map([], |row| row.get(0))
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(albums)
    }

    pub fn list_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT genre FROM songs WHERE genre IS NOT NULL ORDER BY genre COLLATE NOCASE"
        )?;

        let genres = stmt
            .query_map([], |row| row.get(0))
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(genres)
    }

    pub fn list_album_artists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT album_artist FROM songs WHERE album_artist IS NOT NULL ORDER BY album_artist COLLATE NOCASE"
        )?;

        let album_artists = stmt
            .query_map([], |row| row.get(0))
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(album_artists)
    }

    pub fn list_filtered(
        &self,
        tag: &str,
        filter_tag: &str,
        filter_value: &str,
    ) -> Result<Vec<String>> {
        // Map tag names to column names
        let tag_col = match tag.to_lowercase().as_str() {
            "artist" => "artist",
            "album" => "album",
            "albumartist" => "album_artist",
            "genre" => "genre",
            "date" => "date",
            "title" => "title",
            "composer" => "composer",
            "performer" => "performer",
            _ => return Err(RmpdError::Database(format!("unsupported tag: {tag}"))),
        };

        let filter_col = match filter_tag.to_lowercase().as_str() {
            "artist" => "artist",
            "album" => "album",
            "albumartist" => "album_artist",
            "genre" => "genre",
            "date" => "date",
            "title" => "title",
            "composer" => "composer",
            "performer" => "performer",
            _ => {
                return Err(RmpdError::Database(format!(
                    "unsupported filter tag: {filter_tag}"
                )))
            }
        };

        let query = format!(
            "SELECT DISTINCT {tag_col} FROM songs WHERE {filter_col} = ? AND {tag_col} IS NOT NULL ORDER BY {tag_col} COLLATE NOCASE"
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            ?;

        let values = stmt
            .query_map([filter_value], |row| row.get(0))
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(values)
    }

    pub fn find_songs(&self, tag: &str, value: &str) -> Result<Vec<Song>> {
        let query = match tag.to_lowercase().as_str() {
            "artist" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE artist = ?1 ORDER BY album, track",
            "album" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE album = ?1 ORDER BY track",
            "genre" => "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE genre = ?1 ORDER BY artist, album, track",
            _ => return Err(RmpdError::Library(format!("Unsupported tag: {tag}"))),
        };

        let mut stmt = self
            .conn
            .prepare(query)
            ?;

        let songs = stmt
            .query_map(params![value], |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            })
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(songs)
    }

    /// Find songs using filter expression
    pub fn find_songs_filter(
        &self,
        filter_expr: &rmpd_core::filter::FilterExpression,
    ) -> Result<Vec<Song>> {
        let (where_clause, params) = filter_expr.to_sql();

        let query = format!(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs WHERE {where_clause} ORDER BY album, track"
        );

        let mut stmt = self
            .conn
            .prepare(&query)
            ?;

        // Convert Vec<String> to params that rusqlite can use
        let params_refs: Vec<&dyn rusqlite::ToSql> = params
            .iter()
            .map(|s| {
                let r: &dyn rusqlite::ToSql = s;
                r
            })
            .collect();

        let songs = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            })
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(songs)
    }

    pub fn list_all_songs(&self) -> Result<Vec<Song>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, mtime, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
             FROM songs
             ORDER BY path"
        )?;

        let songs = stmt
            .query_map([], |row| {
                Ok(Song {
                    id: row.get::<_, i64>(0)? as u64,
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
                    musicbrainz_trackid: row.get(15)?,
                    musicbrainz_albumid: row.get(16)?,
                    musicbrainz_artistid: row.get(17)?,
                    musicbrainz_albumartistid: row.get(18)?,
                    musicbrainz_releasegroupid: row.get(19)?,
                    musicbrainz_releasetrackid: row.get(20)?,
                    artist_sort: row.get(21)?,
                    album_artist_sort: row.get(22)?,
                    original_date: row.get(23)?,
                    label: row.get(24)?,
                    sample_rate: row.get(25)?,
                    channels: row.get(26)?,
                    bits_per_sample: row.get(27)?,
                    bitrate: row.get(28)?,
                    replay_gain_track_gain: row.get(29)?,
                    replay_gain_track_peak: row.get(30)?,
                    replay_gain_album_gain: row.get(31)?,
                    replay_gain_album_peak: row.get(32)?,
                    added_at: row.get(33)?,
                    last_modified: row.get(34)?,
                })
            })
            ?
            .collect::<std::result::Result<Vec<_>, _>>()
            ?;

        Ok(songs)
    }

    pub fn delete_song_by_path(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM songs WHERE path = ?1", params![path])
            ?;
        Ok(())
    }

    // Artwork methods
    pub fn get_artwork(&self, path: &str, picture_type: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.conn
            .query_row(
                "SELECT data FROM artwork WHERE song_path = ?1 AND picture_type = ?2",
                params![path, picture_type],
                |row| row.get(0),
            )
            .optional()?)
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
        )?;
        Ok(())
    }

    pub fn has_artwork(&self, path: &str, picture_type: &str) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM artwork WHERE song_path = ?1 AND picture_type = ?2",
                params![path, picture_type],
                |row| row.get(0),
            )
            ?;
        Ok(count > 0)
    }

    /// List directory contents (songs + subdirectories)
    pub fn list_directory(&self, path: &str) -> Result<DirectoryListing> {
        // First, find the directory
        let dir_id = if path.is_empty() || path == "/" {
            // Root directory - need to find the music directory root
            // Look for a directory that starts with / and has subdirectories
            // This is a heuristic to find the music directory
            let id: Option<i64> = self.conn.query_row(
                "SELECT id FROM directories WHERE path LIKE '%/Musica' OR path LIKE '%/Music' ORDER BY LENGTH(path) DESC LIMIT 1",
                [],
                |row| row.get(0),
            ).optional()
            ?;

            // If we can't find Music/Musica directory, try to find any directory with songs
            if id.is_none() {
                self.conn
                    .query_row(
                        "SELECT DISTINCT directory_id FROM songs ORDER BY directory_id LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
                    ?
            } else {
                id
            }
        } else {
            // Find directory by path
            let id: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM directories WHERE path = ?1 OR path LIKE '%/' || ?1",
                    params![path],
                    |row| row.get(0),
                )
                .optional()
                ?;
            id
        };

        // Get subdirectories
        let mut directories = Vec::new();
        if let Some(id) = dir_id {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM directories WHERE parent_id = ?1 ORDER BY path")
                ?;
            let rows = stmt
                .query_map(params![id], |row| row.get::<_, String>(0))
                ?;
            for row in rows {
                directories.push(row?);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM directories WHERE parent_id IS NULL ORDER BY path")
                ?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                ?;
            for row in rows {
                directories.push(row?);
            }
        }

        // Get songs in this directory
        let mut songs = Vec::new();
        {
            let query = "SELECT id, path, duration,
                    title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                    musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                    musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                    artist_sort, album_artist_sort, original_date, label,
                    sample_rate, channels, bits_per_sample, bitrate,
                    replay_gain_track_gain, replay_gain_track_peak,
                    replay_gain_album_gain, replay_gain_album_peak,
                    added_at, last_modified
                FROM songs WHERE directory_id = ?1 ORDER BY path";

            let mut stmt = self
                .conn
                .prepare(query)
                ?;

            let song_rows = stmt
                .query_map(params![dir_id.unwrap_or(0)], |row| {
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
                        musicbrainz_trackid: row.get(14).ok(),
                        musicbrainz_albumid: row.get(15).ok(),
                        musicbrainz_artistid: row.get(16).ok(),
                        musicbrainz_albumartistid: row.get(17).ok(),
                        musicbrainz_releasegroupid: row.get(18).ok(),
                        musicbrainz_releasetrackid: row.get(19).ok(),
                        artist_sort: row.get(20).ok(),
                        album_artist_sort: row.get(21).ok(),
                        original_date: row.get(22).ok(),
                        label: row.get(23).ok(),
                        sample_rate: row.get(24).ok(),
                        channels: row.get(25).ok(),
                        bits_per_sample: row.get(26).ok(),
                        bitrate: row.get(27).ok(),
                        replay_gain_track_gain: row.get(28).ok(),
                        replay_gain_track_peak: row.get(29).ok(),
                        replay_gain_album_gain: row.get(30).ok(),
                        replay_gain_album_peak: row.get(31).ok(),
                        added_at: row.get(32)?,
                        last_modified: row.get(33)?,
                    })
                })
                ?;

            for row in song_rows {
                songs.push(row?);
            }
        }

        Ok(DirectoryListing { directories, songs })
    }

    /// List all songs under a directory recursively
    pub fn list_directory_recursive(&self, path: &str) -> Result<Vec<Song>> {
        let query = "SELECT id, path, duration,
                title, artist, album, album_artist, track, disc, date, genre, composer, performer, comment,
                musicbrainz_trackid, musicbrainz_albumid, musicbrainz_artistid, musicbrainz_albumartistid,
                musicbrainz_releasegroupid, musicbrainz_releasetrackid,
                artist_sort, album_artist_sort, original_date, label,
                sample_rate, channels, bits_per_sample, bitrate,
                replay_gain_track_gain, replay_gain_track_peak,
                replay_gain_album_gain, replay_gain_album_peak,
                added_at, last_modified
            FROM songs WHERE path LIKE ?1 || '%' ORDER BY path";

        let mut stmt = self
            .conn
            .prepare(query)
            ?;

        let search_path = if path.is_empty() { "%" } else { path };

        let song_rows = stmt
            .query_map(params![search_path], |row| {
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
                    musicbrainz_trackid: row.get(14).ok(),
                    musicbrainz_albumid: row.get(15).ok(),
                    musicbrainz_artistid: row.get(16).ok(),
                    musicbrainz_albumartistid: row.get(17).ok(),
                    musicbrainz_releasegroupid: row.get(18).ok(),
                    musicbrainz_releasetrackid: row.get(19).ok(),
                    artist_sort: row.get(20).ok(),
                    album_artist_sort: row.get(21).ok(),
                    original_date: row.get(22).ok(),
                    label: row.get(23).ok(),
                    sample_rate: row.get(24).ok(),
                    channels: row.get(25).ok(),
                    bits_per_sample: row.get(26).ok(),
                    bitrate: row.get(27).ok(),
                    replay_gain_track_gain: row.get(28).ok(),
                    replay_gain_track_peak: row.get(29).ok(),
                    replay_gain_album_gain: row.get(30).ok(),
                    replay_gain_album_peak: row.get(31).ok(),
                    added_at: row.get(32)?,
                    last_modified: row.get(33)?,
                })
            })
            ?;

        let mut songs = Vec::new();
        for row in song_rows {
            songs.push(row?);
        }

        Ok(songs)
    }

    /// Save current queue as a playlist
    pub fn save_playlist(&self, name: &str, songs: &[Song]) -> Result<()> {
        // Create or replace playlist
        self.conn
            .execute(
                "INSERT OR REPLACE INTO playlists (name, mtime) VALUES (?1, strftime('%s', 'now'))",
                params![name],
            )
            ?;

        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            ?;

        // Clear existing items
        self.conn
            .execute(
                "DELETE FROM playlist_items WHERE playlist_id = ?1",
                params![playlist_id],
            )
            ?;

        // Add songs
        for (position, song) in songs.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO playlist_items (playlist_id, position, song_id, uri) VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, position as i64, song.id as i64, song.path.as_str()],
            )?;
        }

        Ok(())
    }

    /// Load playlist and return songs
    pub fn load_playlist(&self, name: &str) -> Result<Vec<Song>> {
        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))?;

        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.path, s.duration,
                    s.title, s.artist, s.album, s.album_artist, s.track, s.disc, s.date, s.genre,
                    s.composer, s.performer, s.comment,
                    s.musicbrainz_trackid, s.musicbrainz_albumid, s.musicbrainz_artistid, s.musicbrainz_albumartistid,
                    s.musicbrainz_releasegroupid, s.musicbrainz_releasetrackid,
                    s.artist_sort, s.album_artist_sort, s.original_date, s.label,
                    s.sample_rate, s.channels, s.bits_per_sample, s.bitrate,
                    s.replay_gain_track_gain, s.replay_gain_track_peak,
                    s.replay_gain_album_gain, s.replay_gain_album_peak,
                    s.added_at, s.last_modified
             FROM playlist_items pi
             JOIN songs s ON pi.song_id = s.id
             WHERE pi.playlist_id = ?1
             ORDER BY pi.position"
        )?;

        let song_rows = stmt
            .query_map(params![playlist_id], |row| {
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
                    musicbrainz_trackid: row.get(14).ok(),
                    musicbrainz_albumid: row.get(15).ok(),
                    musicbrainz_artistid: row.get(16).ok(),
                    musicbrainz_albumartistid: row.get(17).ok(),
                    musicbrainz_releasegroupid: row.get(18).ok(),
                    musicbrainz_releasetrackid: row.get(19).ok(),
                    artist_sort: row.get(20).ok(),
                    album_artist_sort: row.get(21).ok(),
                    original_date: row.get(22).ok(),
                    label: row.get(23).ok(),
                    sample_rate: row.get(24).ok(),
                    channels: row.get(25).ok(),
                    bits_per_sample: row.get(26).ok(),
                    bitrate: row.get(27).ok(),
                    replay_gain_track_gain: row.get(28).ok(),
                    replay_gain_track_peak: row.get(29).ok(),
                    replay_gain_album_gain: row.get(30).ok(),
                    replay_gain_album_peak: row.get(31).ok(),
                    added_at: row.get(32)?,
                    last_modified: row.get(33)?,
                })
            })
            ?;

        let mut songs = Vec::new();
        for row in song_rows {
            songs.push(row?);
        }

        Ok(songs)
    }

    /// List all playlists
    pub fn list_playlists(&self) -> Result<Vec<PlaylistInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.name, p.mtime, COUNT(pi.id) as song_count
             FROM playlists p
             LEFT JOIN playlist_items pi ON p.id = pi.playlist_id
             GROUP BY p.id
             ORDER BY p.name",
            )
            ?;

        let playlist_rows = stmt
            .query_map([], |row| {
                Ok(PlaylistInfo {
                    name: row.get(0)?,
                    last_modified: row.get(1)?,
                    song_count: row.get(2)?,
                })
            })
            ?;

        let mut playlists = Vec::new();
        for row in playlist_rows {
            playlists.push(row?);
        }

        Ok(playlists)
    }

    /// Get songs in a playlist
    pub fn get_playlist_songs(&self, name: &str) -> Result<Vec<Song>> {
        self.load_playlist(name)
    }

    /// Delete a playlist
    pub fn delete_playlist(&self, name: &str) -> Result<()> {
        let affected = self
            .conn
            .execute("DELETE FROM playlists WHERE name = ?1", params![name])
            ?;

        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {name}")));
        }

        Ok(())
    }

    /// Rename a playlist
    pub fn rename_playlist(&self, from: &str, to: &str) -> Result<()> {
        let affected = self
            .conn
            .execute(
                "UPDATE playlists SET name = ?1, mtime = strftime('%s', 'now') WHERE name = ?2",
                params![to, from],
            )
            ?;

        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {from}")));
        }

        Ok(())
    }

    /// Add a song to a playlist
    pub fn playlist_add(&self, name: &str, uri: &str) -> Result<()> {
        // Get playlist ID
        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))?;

        // Get song by URI
        let song = self
            .get_song_by_path(uri)?
            .ok_or_else(|| RmpdError::Library(format!("Song not found: {uri}")))?;

        // Get next position
        let next_pos: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
                params![playlist_id],
                |row| row.get(0),
            )
            ?;

        // Add song
        self.conn.execute(
            "INSERT INTO playlist_items (playlist_id, position, song_id, uri) VALUES (?1, ?2, ?3, ?4)",
            params![playlist_id, next_pos, song.id as i64, uri],
        )?;

        // Update mtime
        self.conn
            .execute(
                "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
                params![playlist_id],
            )
            ?;

        Ok(())
    }

    /// Clear all songs from a playlist
    pub fn playlist_clear(&self, name: &str) -> Result<()> {
        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))?;

        self.conn
            .execute(
                "DELETE FROM playlist_items WHERE playlist_id = ?1",
                params![playlist_id],
            )
            ?;

        // Update mtime
        self.conn
            .execute(
                "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
                params![playlist_id],
            )
            ?;

        Ok(())
    }

    /// Delete a song from a playlist by position
    pub fn playlist_delete_pos(&self, name: &str, position: u32) -> Result<()> {
        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))?;

        let affected = self
            .conn
            .execute(
                "DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
                params![playlist_id, position],
            )
            ?;

        if affected == 0 {
            return Err(RmpdError::Library(format!(
                "Position not found: {position}"
            )));
        }

        // Reindex positions
        self.conn
            .execute(
                "UPDATE playlist_items SET position = position - 1
             WHERE playlist_id = ?1 AND position > ?2",
                params![playlist_id, position],
            )
            ?;

        // Update mtime
        self.conn
            .execute(
                "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
                params![playlist_id],
            )
            ?;

        Ok(())
    }

    /// Move a song within a playlist
    pub fn playlist_move(&self, name: &str, from: u32, to: u32) -> Result<()> {
        let playlist_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))?;

        if from == to {
            return Ok(());
        }

        // This is a bit complex - we need to:
        // 1. Get the item at 'from'
        // 2. Delete it
        // 3. Shift positions
        // 4. Insert at 'to'

        let item_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
                params![playlist_id, from],
                |row| row.get(0),
            )
            .optional()
            ?
            .ok_or_else(|| RmpdError::Library(format!("Position not found: {from}")))?;

        // Move logic similar to queue
        if from < to {
            // Moving down: shift items between from+1 and to down by 1
            self.conn
                .execute(
                    "UPDATE playlist_items SET position = position - 1
                 WHERE playlist_id = ?1 AND position > ?2 AND position <= ?3",
                    params![playlist_id, from, to],
                )
                ?;
        } else {
            // Moving up: shift items between to and from-1 up by 1
            self.conn
                .execute(
                    "UPDATE playlist_items SET position = position + 1
                 WHERE playlist_id = ?1 AND position >= ?2 AND position < ?3",
                    params![playlist_id, to, from],
                )
                ?;
        }

        // Set the item's new position
        self.conn
            .execute(
                "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
                params![to, item_id],
            )
            ?;

        // Update mtime
        self.conn
            .execute(
                "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
                params![playlist_id],
            )
            ?;

        Ok(())
    }

    // Sticker methods (arbitrary key-value metadata for songs/directories)

    /// Get a sticker value by URI and name
    pub fn get_sticker(&self, uri: &str, name: &str) -> Result<Option<String>> {
        Ok(self.conn
            .query_row(
                "SELECT value FROM stickers WHERE uri = ?1 AND name = ?2",
                params![uri, name],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Set a sticker value
    pub fn set_sticker(&self, uri: &str, name: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO stickers (uri, name, value) VALUES (?1, ?2, ?3)",
                params![uri, name, value],
            )
            ?;
        Ok(())
    }

    /// Delete sticker(s) for a URI
    /// If name is Some, delete only that sticker
    /// If name is None, delete all stickers for the URI
    pub fn delete_sticker(&self, uri: &str, name: Option<&str>) -> Result<()> {
        if let Some(sticker_name) = name {
            self.conn
                .execute(
                    "DELETE FROM stickers WHERE uri = ?1 AND name = ?2",
                    params![uri, sticker_name],
                )
                ?;
        } else {
            self.conn
                .execute("DELETE FROM stickers WHERE uri = ?1", params![uri])
                ?;
        }
        Ok(())
    }

    /// List all stickers for a URI
    pub fn list_stickers(&self, uri: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, value FROM stickers WHERE uri = ?1 ORDER BY name")
            ?;

        let sticker_rows = stmt
            .query_map(params![uri], |row| Ok((row.get(0)?, row.get(1)?)))
            ?;

        let mut stickers = Vec::new();
        for row in sticker_rows {
            stickers.push(row?);
        }

        Ok(stickers)
    }

    /// Find all URIs that have a sticker with the given name
    /// Returns (uri, value) pairs
    pub fn find_stickers(&self, uri: &str, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uri, value FROM stickers WHERE uri LIKE ?1 AND name = ?2 ORDER BY uri")
            ?;

        let search_pattern = if uri.is_empty() {
            "%".to_string()
        } else {
            format!("{uri}%")
        };

        let sticker_rows = stmt
            .query_map(params![search_pattern, name], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            ?;

        let mut results = Vec::new();
        for row in sticker_rows {
            results.push(row?);
        }

        Ok(results)
    }
}

/// Directory listing result
#[derive(Debug)]
pub struct DirectoryListing {
    pub directories: Vec<String>,
    pub songs: Vec<Song>,
}

/// Playlist information
#[derive(Debug)]
pub struct PlaylistInfo {
    pub name: String,
    pub last_modified: i64,
    pub song_count: u32,
}
