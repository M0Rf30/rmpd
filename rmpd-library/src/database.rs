use camino::Utf8PathBuf;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::Song;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::cmp::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Compare two optional strings using ICU root-locale collation: None sorts before Some.
/// Matches MPD's compare_utf8_string() + IcuCollate() behaviour.
fn icu_cmp_opt(col: &CollatorBorrowed<'_>, a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => col.compare(a, b),
    }
}

/// An entry yielded during a recursive directory walk.
pub enum WalkEntry<'a> {
    /// A song file.
    Song(&'a Song),
    /// A directory (emitted before its contents are visited).
    Directory(&'a str),
}

/// SELECT columns for song audio properties (no tags — those come from song_tags).
const SONG_COLUMNS: &str = "id, path, duration, sample_rate, channels, bits_per_sample, bitrate,
     replay_gain_track_gain, replay_gain_track_peak,
     replay_gain_album_gain, replay_gain_album_peak,
     added_at, last_modified";

/// Same columns with `s.` table alias.
const SONG_COLUMNS_ALIASED: &str =
    "s.id, s.path, s.duration, s.sample_rate, s.channels, s.bits_per_sample, s.bitrate,
     s.replay_gain_track_gain, s.replay_gain_track_peak,
     s.replay_gain_album_gain, s.replay_gain_album_peak,
     s.added_at, s.last_modified";

/// Construct a Song (without tags) from a database row.
/// Tags are loaded separately via `load_tags_for_songs`.
fn song_from_row(row: &Row<'_>) -> rusqlite::Result<Song> {
    Ok(Song {
        id: row.get::<_, i64>(0)? as u64,
        path: row.get::<_, String>(1)?.into(),
        duration: row.get::<_, Option<f64>>(2)?.map(Duration::from_secs_f64),
        sample_rate: row.get(3)?,
        channels: row.get(4)?,
        bits_per_sample: row.get(5)?,
        bitrate: row.get(6)?,
        replay_gain_track_gain: row.get(7)?,
        replay_gain_track_peak: row.get(8)?,
        replay_gain_album_gain: row.get(9)?,
        replay_gain_album_peak: row.get(10)?,
        added_at: row.get(11)?,
        last_modified: row.get(12)?,
        tags: Vec::new(),
    })
}

/// Convert a SystemTime to Unix timestamp (seconds since epoch).
pub(crate) fn system_time_to_unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| {
            tracing::warn!("system time before UNIX_EPOCH, using 0");
            Duration::ZERO
        })
        .as_secs() as i64
}

/// Return the tag fallback chain for a given tag (MPD's Fallback.hxx).
fn tag_fallback_chain(tag: &str) -> Vec<&str> {
    match tag {
        "albumartist" => vec!["albumartist", "artist"],
        "artistsort" => vec!["artistsort", "artist"],
        "albumartistsort" => vec!["albumartistsort", "albumartist", "artistsort", "artist"],
        "albumsort" => vec!["albumsort", "album"],
        "titlesort" => vec!["titlesort", "title"],
        "composersort" => vec!["composersort", "composer"],
        _ => vec![tag],
    }
}

/// Look up a playlist by name and return its ID.
fn get_playlist_id(conn: &Connection, name: &str) -> Result<i64> {
    conn.query_row(
        "SELECT id FROM playlists WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| RmpdError::Library(format!("Playlist not found: {name}")))
}

#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn init_schema(&self) -> Result<()> {
        // Songs table — audio properties only, no tag columns
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS songs (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                directory_id INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                duration REAL,
                sample_rate INTEGER,
                channels INTEGER,
                bits_per_sample INTEGER,
                bitrate INTEGER,
                replay_gain_track_gain REAL,
                replay_gain_track_peak REAL,
                replay_gain_album_gain REAL,
                replay_gain_album_peak REAL,
                added_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                last_modified INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                FOREIGN KEY (directory_id) REFERENCES directories(id)
            )",
            [],
        )?;

        // Normalized tag storage — one row per (song, tag, value) triple
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS song_tags (
                song_id INTEGER NOT NULL REFERENCES songs(id) ON DELETE CASCADE,
                tag TEXT NOT NULL,
                value TEXT NOT NULL DEFAULT '',
                UNIQUE(song_id, tag, value)
            )",
            [],
        )?;

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
        )?;

        // Artists table (normalized)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE
            )",
            [],
        )?;

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
        )?;

        // Playlists table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                mtime INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            )",
            [],
        )?;

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
        )?;

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
        )?;

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
        )?;

        // Full-text search using FTS5 (content-less — we manage sync manually)
        self.conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS songs_fts USING fts5(
                title, artist, album, album_artist, genre, composer,
                content=''
            )",
            [],
        )?;

        // Indexes on song_tags
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_song_tags_tag_value ON song_tags(tag, value)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_song_tags_song_id ON song_tags(song_id)",
            [],
        )?;

        // Indexes on songs
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_songs_directory ON songs(directory_id)",
            [],
        )?;

        // Indexes on directories
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_directories_parent ON directories(parent_id)",
            [],
        )?;

        // Indexes on artwork
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_artwork_path ON artwork(song_path)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_artwork_hash ON artwork(hash)",
            [],
        )?;

        // FTS delete trigger — clean up FTS when songs are deleted
        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS songs_fts_delete AFTER DELETE ON songs BEGIN
                INSERT INTO songs_fts(songs_fts, rowid, title, artist, album, album_artist, genre, composer)
                VALUES ('delete', old.id, '', '', '', '', '', '');
            END",
            [],
        )?;

        Ok(())
    }

    /// Load tags for a single song by id.
    fn load_tags_for_song(&self, song_id: u64) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag, value FROM song_tags WHERE song_id = ?1")?;
        let tags = stmt
            .query_map(params![song_id as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    /// Load tags for a batch of songs in-place.
    fn load_tags_for_songs(&self, songs: &mut [Song]) -> Result<()> {
        if songs.is_empty() {
            return Ok(());
        }
        // For small batches, query per song. For large batches, use a single query.
        if songs.len() <= 10 {
            for song in songs.iter_mut() {
                song.tags = self.load_tags_for_song(song.id)?;
            }
        } else {
            // Build id list and query all tags at once
            let ids: Vec<String> = songs.iter().map(|s| s.id.to_string()).collect();
            let id_list = ids.join(",");
            let sql = format!(
                "SELECT song_id, tag, value FROM song_tags WHERE song_id IN ({id_list}) ORDER BY song_id"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows: Vec<(i64, String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            // Build a map of song_id -> index in songs slice
            let mut id_to_idx: Vec<(u64, usize)> =
                songs.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
            id_to_idx.sort_by_key(|(id, _)| *id);

            for (song_id, tag, value) in rows {
                let sid = song_id as u64;
                if let Ok(pos) = id_to_idx.binary_search_by_key(&sid, |(id, _)| *id) {
                    let idx = id_to_idx[pos].1;
                    songs[idx].tags.push((tag, value));
                }
            }
        }
        Ok(())
    }

    /// Update the FTS index for a song by reading its tags from song_tags.
    fn update_fts_for_song(&self, song_id: u64) -> Result<()> {
        let title: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'title' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let artist: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'artist' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let album: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'album' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let album_artist: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'albumartist' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let genre: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'genre' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let composer: String = self
            .conn
            .query_row(
                "SELECT COALESCE(value, '') FROM song_tags WHERE song_id = ?1 AND tag = 'composer' LIMIT 1",
                params![song_id as i64],
                |row| row.get(0),
            )
            .unwrap_or_default();

        // Delete old FTS entry if it exists, then insert new one
        self.conn.execute(
            "INSERT INTO songs_fts(songs_fts, rowid, title, artist, album, album_artist, genre, composer)
             VALUES ('delete', ?1, '', '', '', '', '', '')",
            params![song_id as i64],
        ).ok(); // ignore error if entry doesn't exist

        self.conn.execute(
            "INSERT INTO songs_fts(rowid, title, artist, album, album_artist, genre, composer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                song_id as i64,
                title,
                artist,
                album,
                album_artist,
                genre,
                composer
            ],
        )?;
        Ok(())
    }

    pub fn add_song(&self, song: &Song) -> Result<u64> {
        let root_path = Utf8PathBuf::from("/");
        let dir_path = song.path.parent().unwrap_or(root_path.as_path());
        let dir_id = self.get_or_create_directory(dir_path)?;

        // Insert or replace the song row (audio properties only)
        self.conn.execute(
            "INSERT OR REPLACE INTO songs (
                path, directory_id, mtime, duration,
                sample_rate, channels, bits_per_sample, bitrate,
                replay_gain_track_gain, replay_gain_track_peak,
                replay_gain_album_gain, replay_gain_album_peak
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                song.path.as_str(),
                dir_id,
                song.last_modified,
                song.duration.map(|d| d.as_secs_f64()),
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

        let song_id = self.conn.last_insert_rowid() as u64;

        // Delete old tags (in case of replace)
        self.conn.execute(
            "DELETE FROM song_tags WHERE song_id = ?1",
            params![song_id as i64],
        )?;

        // Insert all tags
        let mut tag_stmt = self
            .conn
            .prepare("INSERT OR IGNORE INTO song_tags (song_id, tag, value) VALUES (?1, ?2, ?3)")?;
        for (tag, value) in &song.tags {
            tag_stmt.execute(params![song_id as i64, tag, value])?;
        }

        // Update FTS index
        self.update_fts_for_song(song_id)?;

        Ok(song_id)
    }

    pub fn get_song(&self, id: u64) -> Result<Option<Song>> {
        let query = format!("SELECT {SONG_COLUMNS} FROM songs WHERE id = ?1");
        let song = self
            .conn
            .query_row(&query, params![id as i64], song_from_row)
            .optional()?;
        match song {
            Some(mut s) => {
                s.tags = self.load_tags_for_song(s.id)?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub fn get_song_by_path(&self, path: &str) -> Result<Option<Song>> {
        let query = format!("SELECT {SONG_COLUMNS} FROM songs WHERE path = ?1");
        let song = self
            .conn
            .query_row(&query, params![path], song_from_row)
            .optional()?;
        match song {
            Some(mut s) => {
                s.tags = self.load_tags_for_song(s.id)?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub fn count_songs(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))?)
    }

    pub fn count_artists(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT value) FROM song_tags WHERE tag = 'artist' AND value != ''",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn count_albums(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(DISTINCT value) FROM song_tags WHERE tag = 'album' AND value != ''",
            [],
            |row| row.get(0),
        )?)
    }

    /// Get all database statistics in a single query.
    /// Returns (songs, artists, albums, playtime_secs, last_update).
    pub fn get_stats(&self) -> Result<(u32, u32, u32, u64, i64)> {
        let song_count: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))?;
        let artists: u32 = self.count_artists()?;
        let albums: u32 = self.count_albums()?;
        let (playtime, last_update): (f64, i64) = self.conn.query_row(
            "SELECT COALESCE(SUM(duration), 0), COALESCE(MAX(added_at), 0) FROM songs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((song_count, artists, albums, playtime as u64, last_update))
    }

    fn get_or_create_directory(&self, path: &camino::Utf8Path) -> Result<i64> {
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM directories WHERE path = ?1",
                params![path.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(id);
        }

        let parent_id = if let Some(parent) = path.parent() {
            Some(self.get_or_create_directory(parent)?)
        } else {
            None
        };

        let now = system_time_to_unix_secs(SystemTime::now());
        self.conn.execute(
            "INSERT INTO directories (path, parent_id, mtime) VALUES (?1, ?2, ?3)",
            params![path.as_str(), parent_id, now],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Escape FTS5 query to handle special characters and reserved words
    fn escape_fts_query(query: &str) -> String {
        let reserved_words = ["AND", "OR", "NOT", "NEAR"];
        let query_upper = query.to_uppercase();

        let needs_escaping = query.contains('"')
            || query.contains('\'')
            || query.contains('(')
            || query.contains(')')
            || reserved_words.iter().any(|&word| query_upper == word);

        if needs_escaping {
            format!("\"{}\"", query.replace('"', "\"\""))
        } else {
            query.to_string()
        }
    }

    pub fn search_songs(&self, query: &str) -> Result<Vec<Song>> {
        let escaped_query = Self::escape_fts_query(query);
        let sql = format!(
            "SELECT {SONG_COLUMNS_ALIASED}
             FROM songs s
             JOIN songs_fts ON songs_fts.rowid = s.id
             WHERE songs_fts MATCH ?1
             ORDER BY rank"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![escaped_query], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    pub fn list_artists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT value FROM song_tags WHERE tag = 'artist' AND value != '' ORDER BY value COLLATE NOCASE",
        )?;
        let artists = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(artists)
    }

    pub fn list_albums(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT value FROM song_tags WHERE tag = 'album' AND value != '' ORDER BY value COLLATE NOCASE",
        )?;
        let albums = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(albums)
    }

    pub fn list_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT value FROM song_tags WHERE tag = 'genre' AND value != '' ORDER BY value COLLATE NOCASE",
        )?;
        let genres = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(genres)
    }

    pub fn list_album_artists(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT value FROM song_tags WHERE tag = 'albumartist' AND value != '' ORDER BY value COLLATE NOCASE",
        )?;
        let album_artists = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(album_artists)
    }

    /// List unique values for any tag, with MPD-style fallback.
    /// Sorted with ICU root-locale collation to match MPD's IcuCollate().
    pub fn list_tag_values(&self, tag: &str) -> Result<Vec<String>> {
        let tag_lower = tag.to_lowercase();
        let chain = tag_fallback_chain(&tag_lower);

        let mut values: Vec<String> = if chain.len() == 1 {
            // Simple case — single tag
            let query = "SELECT DISTINCT value FROM song_tags WHERE tag = ?1";
            let mut stmt = self.conn.prepare(query)?;
            stmt.query_map(params![chain[0]], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            // Fallback chain: for each song, use the first tag in the chain that has a value.
            // Simplified: UNION all values from each fallback level, excluding songs that
            // already have a value at a higher priority level.
            let primary = chain[0];
            let mut all_values: Vec<String> = Vec::new();

            // Get values from primary tag
            let mut stmt = self
                .conn
                .prepare("SELECT DISTINCT value FROM song_tags WHERE tag = ?1")?;
            let primary_vals: Vec<String> = stmt
                .query_map(params![primary], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            all_values.extend(primary_vals);

            // For each fallback level, get values for songs that don't have the primary tag
            for fallback in &chain[1..] {
                let sql = format!(
                    "SELECT DISTINCT st.value FROM song_tags st
                     WHERE st.tag = ?1
                       AND st.song_id NOT IN (SELECT song_id FROM song_tags WHERE tag = '{primary}')"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let vals: Vec<String> = stmt
                    .query_map(params![fallback], |row| row.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                all_values.extend(vals);
            }

            // Deduplicate
            all_values.sort();
            all_values.dedup();
            all_values
        };

        // Sort with ICU collation to match MPD: empties first, then ICU root-locale order.
        let collator =
            CollatorBorrowed::try_new(CollatorPreferences::default(), Default::default())
                .unwrap_or_else(|_| panic!("ICU collator unavailable"));
        values.sort_by(|a, b| match (a.is_empty(), b.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => collator.compare(a, b),
        });
        Ok(values)
    }

    pub fn list_filtered(
        &self,
        tag: &str,
        filter_tag: &str,
        filter_value: &str,
    ) -> Result<Vec<String>> {
        let tag_lower = tag.to_lowercase();
        let filter_lower = filter_tag.to_lowercase();

        // Find songs matching the filter, then extract the requested tag values
        let sql = "SELECT DISTINCT t1.value FROM song_tags t1
             JOIN song_tags t2 ON t1.song_id = t2.song_id
             WHERE t1.tag = ?1 AND t1.value != ''
               AND t2.tag = ?2 AND t2.value = ?3";
        let mut stmt = self.conn.prepare(sql)?;
        let mut values: Vec<String> = stmt
            .query_map(params![tag_lower, filter_lower, filter_value], |row| {
                row.get(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let collator =
            CollatorBorrowed::try_new(CollatorPreferences::default(), Default::default())
                .unwrap_or_else(|_| panic!("ICU collator unavailable"));
        values.sort_by(|a, b| collator.compare(a, b));
        Ok(values)
    }

    /// Get all songs from the database.
    pub fn get_all_songs(&self) -> Result<Vec<Song>> {
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map([], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    pub fn find_songs(&self, tag: &str, value: &str) -> Result<Vec<Song>> {
        let tag_lower = tag.to_lowercase();
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             WHERE id IN (SELECT song_id FROM song_tags WHERE tag = ?1 AND value = ?2)
             ORDER BY id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![tag_lower, value], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    /// Find songs by exact match across all tag values (for `any` tag).
    pub fn find_songs_any(&self, value: &str) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             WHERE id IN (SELECT song_id FROM song_tags WHERE value = ?1)
                OR path = ?1
             ORDER BY id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![value], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    /// Find songs using filter expression
    pub fn find_songs_filter(
        &self,
        filter_expr: &rmpd_core::filter::FilterExpression,
    ) -> Result<Vec<Song>> {
        let (where_clause, filter_params) = filter_expr.to_sql();

        let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE {where_clause} ORDER BY id");

        let mut stmt = self.conn.prepare(&sql)?;

        let params_refs: Vec<&dyn rusqlite::ToSql> = filter_params
            .iter()
            .map(|s| {
                let r: &dyn rusqlite::ToSql = s;
                r
            })
            .collect();

        let mut songs: Vec<Song> = stmt
            .query_map(params_refs.as_slice(), song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    pub fn list_all_songs(&self) -> Result<Vec<Song>> {
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs ORDER BY path");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map([], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    pub fn delete_song_by_path(&self, path: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM songs WHERE path = ?1", params![path])?;
        Ok(())
    }

    // Artwork methods
    pub fn get_artwork(&self, path: &str, picture_type: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
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
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artwork WHERE song_path = ?1 AND picture_type = ?2",
            params![path, picture_type],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List directory contents (songs + subdirectories)
    pub fn list_directory(&self, path: &str) -> Result<DirectoryListing> {
        let dir_id = self.resolve_dir_id(path)?;

        // Get subdirectories
        let mut directories = Vec::new();
        if let Some(id) = dir_id {
            let mut stmt = self.conn.prepare(
                "SELECT path, mtime FROM directories WHERE parent_id = ?1 ORDER BY path",
            )?;
            let rows = stmt.query_map(params![id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                directories.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT path, mtime FROM directories WHERE parent_id IS NULL ORDER BY path",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                directories.push(row?);
            }
        }

        // Get songs in this directory
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE directory_id = ?1 ORDER BY path");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![dir_id.unwrap_or(0)], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;

        Ok(DirectoryListing { directories, songs })
    }

    /// List all songs under a directory recursively
    pub fn list_directory_recursive(&self, path: &str) -> Result<Vec<Song>> {
        let sql =
            format!("SELECT {SONG_COLUMNS} FROM songs WHERE path LIKE ?1 || '%' ORDER BY path");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![path], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    /// Resolve a path to a directory id, or None if not found.
    fn resolve_dir_id(&self, path: &str) -> Result<Option<i64>> {
        if path.is_empty() || path == "/" {
            Ok(self
                .conn
                .query_row(
                    "SELECT id FROM directories WHERE parent_id IS NULL LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?)
        } else {
            Ok(self
                .conn
                .query_row(
                    "SELECT id FROM directories WHERE path = ?1",
                    params![path],
                    |row| row.get(0),
                )
                .optional()?)
        }
    }

    /// Walk a directory tree recursively in DFS order, matching MPD's traversal.
    pub fn walk_recursive(
        &self,
        path: &str,
        visitor: &mut impl FnMut(WalkEntry<'_>) -> Result<()>,
    ) -> Result<()> {
        let dir_id = self.resolve_dir_id(path)?;
        self.walk_dir(dir_id, visitor)
    }

    fn walk_dir(
        &self,
        dir_id: Option<i64>,
        visitor: &mut impl FnMut(WalkEntry<'_>) -> Result<()>,
    ) -> Result<()> {
        let id = match dir_id {
            Some(id) => id,
            None => return Ok(()),
        };

        // Get songs in this directory
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE directory_id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![id], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;

        // Sort to match MPD's song_cmp: (album NULL-first, disc, track, filename)
        let col = CollatorBorrowed::try_new(CollatorPreferences::default(), Default::default())
            .unwrap_or_else(|_| panic!("ICU collator unavailable"));
        songs.sort_by(|a, b| {
            let album_ord = icu_cmp_opt(&col, a.tag("album"), b.tag("album"));
            if album_ord != Ordering::Equal {
                return album_ord;
            }
            let disc_a: u32 = a.tag("disc").and_then(|v| v.parse().ok()).unwrap_or(0);
            let disc_b: u32 = b.tag("disc").and_then(|v| v.parse().ok()).unwrap_or(0);
            let disc_ord = disc_a.cmp(&disc_b);
            if disc_ord != Ordering::Equal {
                return disc_ord;
            }
            let track_a: u32 = a.tag("track").and_then(|v| v.parse().ok()).unwrap_or(0);
            let track_b: u32 = b.tag("track").and_then(|v| v.parse().ok()).unwrap_or(0);
            let track_ord = track_a.cmp(&track_b);
            if track_ord != Ordering::Equal {
                return track_ord;
            }
            let a_name = a.path.file_name().unwrap_or(a.path.as_str());
            let b_name = b.path.file_name().unwrap_or(b.path.as_str());
            col.compare(a_name, b_name)
        });

        for song in &songs {
            visitor(WalkEntry::Song(song))?;
        }

        // Collect immediate subdirectories, sorted by path
        let mut dir_stmt = self
            .conn
            .prepare("SELECT id, path FROM directories WHERE parent_id = ?1 ORDER BY path")?;
        let subdirs: Vec<(i64, String)> = dir_stmt
            .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        for (child_id, child_path) in &subdirs {
            visitor(WalkEntry::Directory(child_path.as_str()))?;
            self.walk_dir(Some(*child_id), visitor)?;
        }

        Ok(())
    }

    /// Save current queue as a playlist
    pub fn save_playlist(&self, name: &str, songs: &[Song]) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO playlists (name, mtime) VALUES (?1, strftime('%s', 'now'))",
            params![name],
        )?;

        let playlist_id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;

        self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
        )?;

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
        let playlist_id = get_playlist_id(&self.conn, name)?;

        let sql = format!(
            "SELECT {SONG_COLUMNS_ALIASED}
             FROM playlist_items pi
             JOIN songs s ON pi.song_id = s.id
             WHERE pi.playlist_id = ?1
             ORDER BY pi.position"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut songs: Vec<Song> = stmt
            .query_map(params![playlist_id], song_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_tags_for_songs(&mut songs)?;
        Ok(songs)
    }

    /// List all playlists
    pub fn list_playlists(&self) -> Result<Vec<PlaylistInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name, p.mtime, COUNT(pi.id) as song_count
             FROM playlists p
             LEFT JOIN playlist_items pi ON p.id = pi.playlist_id
             GROUP BY p.id
             ORDER BY p.name",
        )?;

        let playlist_rows = stmt.query_map([], |row| {
            Ok(PlaylistInfo {
                name: row.get(0)?,
                last_modified: row.get(1)?,
                song_count: row.get(2)?,
            })
        })?;

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
            .execute("DELETE FROM playlists WHERE name = ?1", params![name])?;
        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {name}")));
        }
        Ok(())
    }

    /// Rename a playlist
    pub fn rename_playlist(&self, from: &str, to: &str) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE playlists SET name = ?1, mtime = strftime('%s', 'now') WHERE name = ?2",
            params![to, from],
        )?;
        if affected == 0 {
            return Err(RmpdError::Library(format!("Playlist not found: {from}")));
        }
        Ok(())
    }

    /// Add a song to a playlist
    pub fn playlist_add(&self, name: &str, uri: &str) -> Result<()> {
        let playlist_id = get_playlist_id(&self.conn, name)?;

        let song = self
            .get_song_by_path(uri)?
            .ok_or_else(|| RmpdError::Library(format!("Song not found: {uri}")))?;

        let next_pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;

        self.conn.execute(
            "INSERT INTO playlist_items (playlist_id, position, song_id, uri) VALUES (?1, ?2, ?3, ?4)",
            params![playlist_id, next_pos, song.id as i64, uri],
        )?;

        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        )?;

        Ok(())
    }

    /// Clear all songs from a playlist
    pub fn playlist_clear(&self, name: &str) -> Result<()> {
        let playlist_id = get_playlist_id(&self.conn, name)?;

        self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
        )?;

        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        )?;

        Ok(())
    }

    /// Delete a song from a playlist by position
    pub fn playlist_delete_pos(&self, name: &str, position: u32) -> Result<()> {
        let playlist_id = get_playlist_id(&self.conn, name)?;

        let affected = self.conn.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
            params![playlist_id, position],
        )?;

        if affected == 0 {
            return Err(RmpdError::Library(format!(
                "Position not found: {position}"
            )));
        }

        self.conn.execute(
            "UPDATE playlist_items SET position = position - 1
             WHERE playlist_id = ?1 AND position > ?2",
            params![playlist_id, position],
        )?;

        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        )?;

        Ok(())
    }

    /// Move a song within a playlist
    pub fn playlist_move(&self, name: &str, from: u32, to: u32) -> Result<()> {
        let playlist_id = get_playlist_id(&self.conn, name)?;

        if from == to {
            return Ok(());
        }

        let item_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
                params![playlist_id, from],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| RmpdError::Library(format!("Position not found: {from}")))?;

        if from < to {
            self.conn.execute(
                "UPDATE playlist_items SET position = position - 1
                 WHERE playlist_id = ?1 AND position > ?2 AND position <= ?3",
                params![playlist_id, from, to],
            )?;
        } else {
            self.conn.execute(
                "UPDATE playlist_items SET position = position + 1
                 WHERE playlist_id = ?1 AND position >= ?2 AND position < ?3",
                params![playlist_id, to, from],
            )?;
        }

        self.conn.execute(
            "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
            params![to, item_id],
        )?;

        self.conn.execute(
            "UPDATE playlists SET mtime = strftime('%s', 'now') WHERE id = ?1",
            params![playlist_id],
        )?;

        Ok(())
    }

    // Sticker methods

    pub fn get_sticker(&self, uri: &str, name: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM stickers WHERE uri = ?1 AND name = ?2",
                params![uri, name],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_sticker(&self, uri: &str, name: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stickers (uri, name, value) VALUES (?1, ?2, ?3)",
            params![uri, name, value],
        )?;
        Ok(())
    }

    pub fn delete_sticker(&self, uri: &str, name: Option<&str>) -> Result<()> {
        if let Some(sticker_name) = name {
            self.conn.execute(
                "DELETE FROM stickers WHERE uri = ?1 AND name = ?2",
                params![uri, sticker_name],
            )?;
        } else {
            self.conn
                .execute("DELETE FROM stickers WHERE uri = ?1", params![uri])?;
        }
        Ok(())
    }

    pub fn list_stickers(&self, uri: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, value FROM stickers WHERE uri = ?1 ORDER BY name")?;
        let sticker_rows = stmt.query_map(params![uri], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut stickers = Vec::new();
        for row in sticker_rows {
            stickers.push(row?);
        }
        Ok(stickers)
    }

    pub fn find_stickers(&self, uri: &str, name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT uri, value FROM stickers WHERE uri LIKE ?1 AND name = ?2 ORDER BY uri",
        )?;

        let search_pattern = if uri.is_empty() {
            "%".to_string()
        } else {
            format!("{uri}%")
        };

        let sticker_rows = stmt.query_map(params![search_pattern, name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

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
    pub directories: Vec<(String, i64)>,
    pub songs: Vec<Song>,
}

/// Playlist information
#[derive(Debug)]
pub struct PlaylistInfo {
    pub name: String,
    pub last_modified: i64,
    pub song_count: u32,
}
