// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `SubsonicSource` — OpenSubsonic/Subsonic REST API backend.
//!
//! Compiled only when `feature = "subsonic"` is active (declared in `lib.rs`).
//! No network I/O occurs at construction time; the client is validated lazily on `ping`.

use async_trait::async_trait;
use camino::Utf8PathBuf;
use futures::StreamExt;
use opensubsonic::{Auth, Client, Error as SubsonicError, SubsonicApiError};
use rmpd_core::config::SourceConfig;
use rmpd_core::song::{Song, intern_tag_key};
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult};

// ─── Percent-encoding helper ─────────────────────────────────────────────────

/// Percent-encode characters that would break the virtual path scheme.
///
/// At minimum `%` → `%25` and `/` → `%2F`, so that splitting on `/` and taking
/// the trailing segment always yields the unmodified Subsonic song id.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            _ => out.push(c),
        }
    }
    out
}

/// Derive a file extension for the virtual path so clients can infer the codec
/// (e.g. rmpc shows "flac"). Prefer the server-reported `suffix`; when absent,
/// map the MIME `content_type`. The extension MUST be one of
/// `rmpd_source`'s known audio extensions so `resolve_stream_uri` can strip it
/// back off to recover the bare id.
fn file_ext_for(c: &opensubsonic::data::Child) -> Option<String> {
    if let Some(s) = c.suffix.as_deref()
        && !s.is_empty()
    {
        return Some(s.to_ascii_lowercase());
    }
    c.content_type
        .as_deref()
        .and_then(mime_to_ext)
        .map(str::to_owned)
}

/// Map a common audio MIME type to a file extension. Returns `None` for
/// unrecognized types (no extension is appended then).
fn mime_to_ext(mime: &str) -> Option<&'static str> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    Some(match base.as_str() {
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg-3" => "mp3",
        "audio/ogg" | "application/ogg" | "audio/vorbis" => "ogg",
        "audio/opus" => "opus",
        "audio/aac" | "audio/aacp" => "aac",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/x-ape" | "audio/ape" | "audio/x-monkeys-audio" => "ape",
        "audio/x-wavpack" | "audio/wavpack" => "wv",
        "audio/x-ms-wma" => "wma",
        "audio/aiff" | "audio/x-aiff" => "aiff",
        "audio/dsf" | "audio/x-dsf" => "dsf",
        _ => return None,
    })
}

// ─── Error mapping ───────────────────────────────────────────────────────────

/// Map opensubsonic errors to `SourceError` without leaking credentials.
///
/// Auth error codes (40–44) map to `SourceError::Auth`; transport failures to
/// `SourceError::Unreachable`; everything else to `SourceError::Protocol`.
pub fn map_err(e: SubsonicError) -> SourceError {
    match e {
        SubsonicError::Http(e) => SourceError::Unreachable(e.to_string()),
        SubsonicError::Api(SubsonicApiError { code, message, .. }) => match code {
            // 40 WrongCredentials, 41 TokenAuthNotSupported,
            // 42 AuthMechanismNotSupported, 43 ConflictingAuthentication,
            // 44 InvalidApiKey
            40..=44 => SourceError::Auth(message),
            _ => SourceError::Protocol(message),
        },
        SubsonicError::Url(e) => SourceError::Protocol(e.to_string()),
        SubsonicError::Parse(msg) => SourceError::Protocol(msg),
        SubsonicError::Other(msg) => SourceError::Protocol(msg),
    }
}

// ─── Layout ──────────────────────────────────────────────────────────────────

/// How a Subsonic source's catalog is laid out under its mount point.
///
/// Either way a song's leaf stays `<id>[.<ext>]`: the id must round-trip
/// through the path for streaming and cover art.
///
/// The ad-hoc results of [`lookup`](MusicSource::lookup),
/// [`search`](MusicSource::search) and
/// [`playlist_items`](MusicSource::playlist_items) always use the `Tags`
/// grammar: the server cannot map a song id back to its folder chain without
/// walking `getMusicDirectory` upwards, and only the leaf id matters for
/// playback. The synced database (and [`browse`](MusicSource::browse)) follow
/// the configured layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Virtual `<name>/<Artist>/<Album>/<id>.<ext>` tree from the ID3 tags
    /// (`getAlbumList2` + `getAlbum`).
    #[default]
    Tags,
    /// The server's own directory hierarchy
    /// `<name>/<folder>/.../<id>.<ext>` (`getIndexes` + `getMusicDirectory`).
    Folders,
}

impl Layout {
    /// Parse the `layout` setting (case-insensitive).
    ///
    /// # Errors
    /// Any value other than `tags` / `folders` → `SourceError::Config`.
    fn parse(value: &str) -> Result<Self, SourceError> {
        match value.to_ascii_lowercase().as_str() {
            "tags" => Ok(Self::Tags),
            "folders" => Ok(Self::Folders),
            other => Err(SourceError::Config(format!(
                "subsonic source `layout` must be \"tags\" or \"folders\", got \"{other}\""
            ))),
        }
    }
}

// ─── Config ──────────────────────────────────────────────────────────────────

/// Validated, desugared view of a `[[source]]` config block for a Subsonic backend.
pub struct SubsonicConfig {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_key: Option<String>,
    pub max_bitrate: Option<u32>,
    pub format: Option<String>,
    pub accept_invalid_certs: bool,
    pub layout: Layout,
}

/// Hand-written `Debug` that redacts credentials (`password`, `api_key`) so a
/// `{:?}` of a config or a `Result<SubsonicConfig, _>` can never leak secrets
/// into logs or panic messages. Mirrors `rmpd_core::config::SourceConfig`.
impl std::fmt::Debug for SubsonicConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubsonicConfig")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("max_bitrate", &self.max_bitrate)
            .field("format", &self.format)
            .field("accept_invalid_certs", &self.accept_invalid_certs)
            .field("layout", &self.layout)
            .finish()
    }
}

impl SubsonicConfig {
    /// Parse and validate settings from a `[[source]]` config block.
    ///
    /// # Errors
    /// - `url` is absent → `SourceError::Config`
    /// - Neither `api_key` nor (`username` + `password`) is present → `SourceError::Config`
    /// - `layout` is set to anything but `tags` / `folders` → `SourceError::Config`
    pub fn from_source_config(cfg: &SourceConfig) -> Result<Self, SourceError> {
        let url = cfg.setting_str("url").ok_or_else(|| {
            SourceError::Config("subsonic source requires a `url` setting".to_owned())
        })?;

        let api_key = cfg.setting_str("api_key");
        let username = cfg.setting_str("username");
        let password = cfg.setting_str("password");

        if api_key.is_none() && (username.is_none() || password.is_none()) {
            return Err(SourceError::Config(
                "subsonic source requires either `api_key` or both `username` and `password`"
                    .to_owned(),
            ));
        }

        let max_bitrate = cfg
            .setting_str("max_bitrate")
            .and_then(|s| s.parse::<u32>().ok());

        let format = cfg.setting_str("format");

        let accept_invalid_certs = cfg
            .setting_str("accept_invalid_certs")
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or(false);

        let layout = cfg
            .setting_str("layout")
            .map_or(Ok(Layout::default()), |v| Layout::parse(&v))?;

        Ok(Self {
            name: cfg.name.clone(),
            url,
            username,
            password,
            api_key,
            max_bitrate,
            format,
            accept_invalid_certs,
            layout,
        })
    }
}

// ─── Source struct ───────────────────────────────────────────────────────────

/// A music source backed by a Subsonic / OpenSubsonic server.
pub struct SubsonicSource {
    name: String,
    client: Client,
    max_bitrate: Option<u32>,
    format: Option<String>,
    layout: Layout,
}

// ─── Factory ─────────────────────────────────────────────────────────────────

/// Setting keys accepted in a `[[source]] type = "subsonic"` block.
pub const SETTINGS: &[&str] = &[
    "url",
    "username",
    "password",
    "api_key",
    "max_bitrate",
    "format",
    "accept_invalid_certs",
    "layout",
];

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS` under `feature = "subsonic"`.
pub fn subsonic_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let sc = SubsonicConfig::from_source_config(cfg)?;

    // Build the HTTP client first so we can set TLS options before handing it to the
    // Subsonic client, avoiding the double-construction `with_danger_accept_invalid_certs`
    // would cause.
    let http = if sc.accept_invalid_certs {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| SourceError::Config(format!("cannot build HTTP client: {e}")))?
    } else {
        reqwest::Client::new()
    };

    let auth = if let Some(key) = sc.api_key {
        Auth::api_key(key)
    } else {
        // `from_source_config` guarantees both are `Some` when `api_key` is
        // absent; still return an error instead of unwrapping in case that
        // invariant is ever violated.
        let username = sc
            .username
            .ok_or_else(|| SourceError::Config("subsonic source missing `username`".to_owned()))?;
        let password = sc
            .password
            .ok_or_else(|| SourceError::Config("subsonic source missing `password`".to_owned()))?;
        Auth::token(username, password)
    };

    let client = Client::new(&sc.url, auth)
        .map_err(|e| SourceError::Config(format!("invalid Subsonic base URL: {e}")))?
        .with_client_name("rmpd")
        .with_http_client(http);

    Ok(Box::new(SubsonicSource {
        name: sc.name,
        client,
        max_bitrate: sc.max_bitrate,
        format: sc.format,
        layout: sc.layout,
    }))
}

// ─── Song mapping ────────────────────────────────────────────────────────────

impl SubsonicSource {
    /// Convert a Subsonic `Child` to an `rmpd_core::song::Song` with a virtual,
    /// MPD mount-style path.
    ///
    /// Path grammar (no `scheme://` — a synced source appears under a plain
    /// top-level mount directory, exactly like MPD's mounted remote storage):
    /// ```text
    /// <source-name>/<enc(artist)>/<enc(album)>/<child.id>[.<suffix>]
    /// ```
    /// `enc()` percent-encodes `%` and `/` so that splitting on `/` and taking
    /// the trailing segment always recovers the `child.id`. The id itself is
    /// left unencoded (Subsonic ids are URL-safe tokens); a trailing
    /// `.<suffix>` (from `Child.suffix`, lower-cased) lets clients infer the
    /// codec/format. `SourceRegistry::resolve_stream_uri` strips that extension
    /// to recover the bare id.
    fn map_song(&self, c: &opensubsonic::data::Child) -> Song {
        self.map_song_with_fallback(c, None, None)
    }

    /// Like [`map_song`](Self::map_song), but fills in artist/album from
    /// `fallback_artist`/`fallback_album` when `c` omits them (some servers
    /// only populate those on `getSong`/`search3`, not on `getAlbum`
    /// children). Resolves the fallback inline instead of cloning `c`.
    fn map_song_with_fallback(
        &self,
        c: &opensubsonic::data::Child,
        fallback_artist: Option<&str>,
        fallback_album: Option<&str>,
    ) -> Song {
        let artist_ref = c.artist.as_deref().or(fallback_artist);
        let album_ref = c.album.as_deref().or(fallback_album);
        let artist = artist_ref.unwrap_or("Unknown Artist");
        let album = album_ref.unwrap_or("Unknown Album");

        let dir = format!("{}/{}/{}", self.name, enc(artist), enc(album));
        build_song(c, &dir, artist_ref, album_ref)
    }

    /// Map a track found while walking the server's directory tree.
    /// `dir_path` is the already-encoded virtual path of its directory
    /// (`<name>/<folder>/...`); the tags are the server's, without the
    /// "Unknown Artist/Album" placeholders the tag layout needs for its path.
    fn map_folder_song(&self, c: &opensubsonic::data::Child, dir_path: &str) -> Song {
        build_song(c, dir_path, c.artist.as_deref(), c.album.as_deref())
    }
}

/// Build a `Song` whose virtual path is `<dir>/<id>[.<ext>]`.
///
/// The leaf is the raw id plus a file extension (from the server `suffix`, or
/// derived from the MIME `content_type`) so clients can infer the codec and
/// `resolve_stream_uri` can recover the id.
fn build_song(
    c: &opensubsonic::data::Child,
    dir: &str,
    artist_ref: Option<&str>,
    album_ref: Option<&str>,
) -> Song {
    let leaf = match file_ext_for(c) {
        Some(ext) => format!("{}.{}", c.id, ext),
        None => c.id.clone(),
    };
    let path = Utf8PathBuf::from(format!("{dir}/{leaf}"));

    let mut tags: Vec<(std::borrow::Cow<'static, str>, String)> = Vec::new();

    // title is always present on Child
    tags.push((intern_tag_key("title"), c.title.clone()));

    if let Some(a) = artist_ref {
        tags.push((intern_tag_key("artist"), a.to_owned()));
        // albumartist defaults to artist when no dedicated field is available
        tags.push((intern_tag_key("albumartist"), a.to_owned()));
    }
    if let Some(alb) = album_ref {
        tags.push((intern_tag_key("album"), alb.to_owned()));
    }
    if let Some(t) = c.track {
        tags.push((intern_tag_key("track"), t.to_string()));
    }
    if let Some(y) = c.year {
        tags.push((intern_tag_key("date"), y.to_string()));
    }
    if let Some(g) = &c.genre {
        tags.push((intern_tag_key("genre"), g.clone()));
    }
    if let Some(d) = c.disc_number {
        tags.push((intern_tag_key("disc"), d.to_string()));
    }

    Song {
        id: 0,
        path,
        duration: c
            .duration
            .map(|secs| std::time::Duration::from_secs(secs as u64)),
        sample_rate: c.sampling_rate.map(|r| r as u32),
        channels: c.channel_count.map(|ch| ch as u8),
        bits_per_sample: c.bit_depth.map(|d| d as u16),
        bitrate: c.bit_rate.map(|b| b as u32),
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
        range: None,
        tags,
    }
}

impl SubsonicSource {
    /// Like [`map_song`](Self::map_song), but fills in artist/album from the
    /// containing album when a `getAlbum` child omits them (some servers only
    /// populate those on `getSong`/`search3`).
    fn map_album_song(
        &self,
        c: &opensubsonic::data::Child,
        fallback_artist: Option<&str>,
        fallback_album: Option<&str>,
    ) -> Song {
        self.map_song_with_fallback(c, fallback_artist, fallback_album)
    }
}

// ─── Lazy-browse helpers (pure) ──────────────────────────────────────────────

/// Decode `%XX` escapes (inverse of [`enc`]); malformed escapes pass through.
fn dec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Split a source-relative directory into decoded, non-empty segments.
fn dir_segments(dir: &str) -> Vec<String> {
    dir.split('/').filter(|s| !s.is_empty()).map(dec).collect()
}

/// Id of the first artist whose name equals `name`.
fn find_artist_id<'a>(artists: &'a opensubsonic::data::ArtistsId3, name: &str) -> Option<&'a str> {
    artists
        .index
        .iter()
        .flat_map(|idx| idx.artist.iter())
        .find(|a| a.name == name)
        .map(|a| a.id.as_str())
}

/// Ids of every album named `name` (duplicate names are merged).
fn matching_album_ids<'a>(albums: &'a [opensubsonic::data::AlbumId3], name: &str) -> Vec<&'a str> {
    albums
        .iter()
        .filter(|a| a.name == name)
        .map(|a| a.id.as_str())
        .collect()
}

impl SubsonicSource {
    /// One `Dir` per artist, as full virtual paths.
    fn artist_entries(&self, artists: &opensubsonic::data::ArtistsId3) -> Vec<SourceEntry> {
        artists
            .index
            .iter()
            .flat_map(|idx| idx.artist.iter())
            .map(|a| SourceEntry::Dir(format!("{}/{}", self.name, enc(&a.name))))
            .collect()
    }

    /// One `Dir` per album under `artist`.
    fn album_entries(
        &self,
        artist: &str,
        albums: &[opensubsonic::data::AlbumId3],
    ) -> Vec<SourceEntry> {
        albums
            .iter()
            .map(|al| SourceEntry::Dir(format!("{}/{}/{}", self.name, enc(artist), enc(&al.name))))
            .collect()
    }

    /// Songs of an album, mapped identically to `list_all`.
    fn album_song_entries(&self, a: &opensubsonic::data::AlbumWithSongsId3) -> Vec<SourceEntry> {
        a.song
            .iter()
            .map(|c| {
                SourceEntry::Song(self.map_album_song(
                    c,
                    a.artist.as_deref(),
                    Some(a.name.as_str()),
                ))
            })
            .collect()
    }
}

// ─── Folder layout (getIndexes / getMusicDirectory) ──────────────────────────

/// Encode a server directory name as one virtual path segment: [`enc`], plus
/// the segments the library cannot store verbatim — an empty name becomes
/// `_`, and `.` / `..` are percent-escaped so they never act as path steps
/// (`dec` restores the dots).
fn folder_seg(name: &str) -> String {
    match name {
        "" => "_".to_owned(),
        "." => "%2E".to_owned(),
        ".." => "%2E%2E".to_owned(),
        _ => enc(name),
    }
}

/// One server directory listing, split into its tracks (already mapped to
/// virtual paths) and its sub-directories as `(virtual path, server id)`.
#[derive(Default)]
struct FolderLevel {
    songs: Vec<Song>,
    dirs: Vec<(String, String)>,
}

impl FolderLevel {
    fn into_entries(self) -> Vec<SourceEntry> {
        self.dirs
            .into_iter()
            .map(|(path, _)| SourceEntry::Dir(path))
            .chain(self.songs.into_iter().map(SourceEntry::Song))
            .collect()
    }
}

impl SubsonicSource {
    /// Split `children` of the directory at virtual path `dir_path` into
    /// tracks and sub-directories. Videos are skipped (the tag layout only
    /// ever lists music); a sub-directory is named by its `title`.
    fn split_children(
        &self,
        dir_path: &str,
        children: &[opensubsonic::data::Child],
    ) -> FolderLevel {
        let mut level = FolderLevel::default();
        for c in children {
            if c.is_dir {
                level
                    .dirs
                    .push((format!("{dir_path}/{}", folder_seg(&c.title)), c.id.clone()));
            } else if c.is_video != Some(true) {
                level.songs.push(self.map_folder_song(c, dir_path));
            }
        }
        level
    }

    /// The top level of the folder tree from `getIndexes`: every indexed
    /// artist entry is a root directory, and `child` entries are loose files
    /// (or directories) directly under the music folder. `shortcut` entries
    /// only repeat indexed directories and are ignored.
    fn split_indexes(&self, indexes: &opensubsonic::data::Indexes) -> FolderLevel {
        let mut level = self.split_children(&self.name, &indexes.child);
        level.dirs.extend(
            indexes
                .index
                .iter()
                .flat_map(|idx| idx.artist.iter())
                .map(|a| {
                    (
                        format!("{}/{}", self.name, folder_seg(&a.name)),
                        a.id.clone(),
                    )
                }),
        );
        level
    }

    /// Walk the whole server hierarchy breadth-first, one level of
    /// `getMusicDirectory` requests at a time with bounded concurrency. A
    /// directory that fails to load is logged and skipped (like a failing
    /// album in the tag layout); each directory id is visited once, so a
    /// server that links a directory under two parents cannot loop.
    async fn list_folders(&self) -> SourceResult<Vec<Song>> {
        /// Concurrent in-flight `getMusicDirectory` requests.
        const CONCURRENCY: usize = 8;

        let indexes = self.client.get_indexes(None, None).await.map_err(map_err)?;
        let FolderLevel {
            mut songs,
            dirs: mut frontier,
        } = self.split_indexes(&indexes);
        let mut seen: std::collections::HashSet<String> =
            frontier.iter().map(|(_, id)| id.clone()).collect();

        while !frontier.is_empty() {
            let listed: Vec<_> = futures::stream::iter(frontier)
                .map(|(path, id)| async move {
                    let dir = self.client.get_music_directory(&id).await;
                    (path, id, dir)
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await;

            let mut next = Vec::new();
            for (path, id, dir) in listed {
                match dir {
                    Ok(dir) => {
                        let level = self.split_children(&path, &dir.child);
                        songs.extend(level.songs);
                        next.extend(
                            level
                                .dirs
                                .into_iter()
                                .filter(|(_, id)| seen.insert(id.clone())),
                        );
                    }
                    Err(e) => tracing::warn!(
                        "subsonic source '{}': skipping directory {id} ({})",
                        self.name,
                        map_err(e)
                    ),
                }
            }
            frontier = next;
        }
        Ok(songs)
    }

    /// One level of the folder tree, for source-relative `dir`. The path is
    /// resolved top-down by name (same-named siblings are merged, as in the
    /// synced tree), one `getMusicDirectory` per component.
    async fn browse_folders(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        let indexes = self.client.get_indexes(None, None).await.map_err(map_err)?;
        let mut level = self.split_indexes(&indexes);
        let mut path = self.name.clone();
        for seg in dir_segments(dir) {
            path = format!("{path}/{}", folder_seg(&seg));
            let matched: Vec<&(String, String)> =
                level.dirs.iter().filter(|(p, _)| *p == path).collect();
            if matched.is_empty() {
                return Err(SourceError::NotFound(format!("subsonic directory: {dir}")));
            }
            let mut next = FolderLevel::default();
            for (p, id) in matched {
                let listing = self.client.get_music_directory(id).await.map_err(map_err)?;
                let sub = self.split_children(p, &listing.child);
                next.songs.extend(sub.songs);
                next.dirs.extend(sub.dirs);
            }
            level = next;
        }
        Ok(level.into_entries())
    }
}

// ─── MusicSource impl ────────────────────────────────────────────────────────

#[async_trait]
impl MusicSource for SubsonicSource {
    fn scheme(&self) -> &str {
        "subsonic"
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Verify connectivity by issuing a `ping` request.
    async fn ping(&self) -> SourceResult<()> {
        self.client.ping().await.map_err(map_err)
    }

    /// Lazily browse the remote library (source-relative `dir`):
    ///
    /// - `""` → one `Dir` per artist (`getArtists`)
    /// - `<artist>` → one `Dir` per album (`getArtist`)
    /// - `<artist>/<album>` → the album's songs (`getAlbum`), mapped exactly like
    ///   `list_all` so browsed songs carry the same virtual paths as synced ones
    ///
    /// Directory entries are full virtual paths (`<source>/<enc(artist)>[/<enc(album)>]`).
    ///
    /// With `layout = "folders"` this instead lists the server's own
    /// directories and tracks (`getIndexes` / `getMusicDirectory`).
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        if self.layout == Layout::Folders {
            return self.browse_folders(dir).await;
        }
        let segs = dir_segments(dir);
        match segs.as_slice() {
            [] => {
                let artists = self.client.get_artists(None).await.map_err(map_err)?;
                Ok(self.artist_entries(&artists))
            }
            [artist] => {
                let artists = self.client.get_artists(None).await.map_err(map_err)?;
                let id = find_artist_id(&artists, artist)
                    .ok_or_else(|| SourceError::NotFound(format!("artist: {artist}")))?;
                let detail = self.client.get_artist(id).await.map_err(map_err)?;
                Ok(self.album_entries(artist, &detail.album))
            }
            [artist, album] => {
                let artists = self.client.get_artists(None).await.map_err(map_err)?;
                let id = find_artist_id(&artists, artist)
                    .ok_or_else(|| SourceError::NotFound(format!("artist: {artist}")))?;
                let detail = self.client.get_artist(id).await.map_err(map_err)?;
                let ids = matching_album_ids(&detail.album, album);
                if ids.is_empty() {
                    return Err(SourceError::NotFound(format!("album: {album}")));
                }
                let mut out = Vec::new();
                for album_id in ids {
                    let a = self.client.get_album(album_id).await.map_err(map_err)?;
                    out.extend(self.album_song_entries(&a));
                }
                Ok(out)
            }
            _ => Err(SourceError::NotFound(format!("subsonic directory: {dir}"))),
        }
    }

    /// Resolve a virtual path (or bare id) to a single song via `getSong`.
    async fn lookup(&self, uri: &str) -> SourceResult<Option<Song>> {
        let id = crate::extract_remote_id(uri);
        if id.is_empty() {
            return Ok(None);
        }
        match self.client.get_song(id).await {
            Ok(c) => Ok(Some(self.map_song(&c))),
            Err(SubsonicError::Api(SubsonicApiError { code: 70, .. })) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Names of the server-side playlists (`getPlaylists`).
    async fn playlists(&self) -> SourceResult<Vec<String>> {
        let lists = self.client.get_playlists(None).await.map_err(map_err)?;
        Ok(lists.into_iter().map(|p| p.name).collect())
    }

    /// Songs of the playlist called `name` (`getPlaylists` + `getPlaylist`).
    async fn playlist_items(&self, name: &str) -> SourceResult<Vec<Song>> {
        let lists = self.client.get_playlists(None).await.map_err(map_err)?;
        let id = lists
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.id.clone())
            .ok_or_else(|| SourceError::NotFound(format!("playlist: {name}")))?;
        let pl = self.client.get_playlist(&id).await.map_err(map_err)?;
        Ok(pl.entry.iter().map(|c| self.map_song(c)).collect())
    }

    /// Enumerate the entire catalog.
    ///
    /// Albums are listed via paginated `getAlbumList2` (one cheap request per
    /// 500 albums, instead of a `getArtists` + per-artist `getArtist` walk),
    /// then each album's songs are fetched with bounded concurrency. A single
    /// album that fails to load is logged and skipped rather than aborting the
    /// whole sync.
    ///
    /// With `layout = "folders"` the catalog is instead the server's directory
    /// tree, walked through `getIndexes` + `getMusicDirectory` (see
    /// [`list_folders`](SubsonicSource::list_folders)).
    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        use opensubsonic::AlbumListType;

        if self.layout == Layout::Folders {
            return self.list_folders().await;
        }

        /// Albums requested per `getAlbumList2` page (Subsonic caps `size` at 500).
        const PAGE: i32 = 500;
        /// Concurrent in-flight `getAlbum` requests.
        const CONCURRENCY: usize = 8;

        // 1. Page through every album stub (cheap; no songs yet).
        let mut albums: Vec<opensubsonic::data::AlbumId3> = Vec::new();
        let mut offset = 0i32;
        loop {
            let page = self
                .client
                .get_album_list2(
                    AlbumListType::AlphabeticalByName,
                    Some(PAGE),
                    Some(offset),
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .map_err(map_err)?;
            let n = page.len() as i32;
            albums.extend(page);
            if n < PAGE {
                break;
            }
            offset += PAGE;
        }

        // 2. Fetch each album's songs concurrently, mapping to rmpd songs.
        let songs: Vec<Song> = futures::stream::iter(albums)
            .map(|album| async move {
                match self.client.get_album(&album.id).await {
                    Ok(detail) => {
                        let fb_artist = detail.artist.clone();
                        let fb_album = Some(detail.name.clone());
                        detail
                            .song
                            .iter()
                            .map(|c| {
                                self.map_album_song(c, fb_artist.as_deref(), fb_album.as_deref())
                            })
                            .collect::<Vec<Song>>()
                    }
                    Err(e) => {
                        tracing::warn!(
                            "subsonic source '{}': skipping album {} ({})",
                            self.name,
                            album.id,
                            map_err(e)
                        );
                        Vec::new()
                    }
                }
            })
            .buffer_unordered(CONCURRENCY)
            .collect::<Vec<Vec<Song>>>()
            .await
            .into_iter()
            .flatten()
            .collect();

        Ok(songs)
    }

    /// Search the server for up to 100 songs matching `query`.
    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let results = self
            .client
            .search3(query, None, None, None, None, Some(100), None, None)
            .await
            .map_err(map_err)?;

        Ok(results.song.iter().map(|c| self.map_song(c)).collect())
    }

    /// Build a direct streaming URL for `song_id` (no network I/O).
    ///
    /// `song_id` is the raw Subsonic id extracted from the virtual path's
    /// trailing segment by `SourceRegistry::resolve_stream_uri`.
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        self.client
            .stream_url(
                song_id,
                self.max_bitrate.map(|b| b as i32),
                self.format.as_deref(),
            )
            .map(|u| u.to_string())
            .map_err(map_err)
    }

    /// Fetch cover art from the server via `getCoverArt`. Subsonic resolves a
    /// song id to its album/track artwork. A missing/erroring art request is
    /// treated as "no art" (`Ok(None)`) rather than a hard failure.
    async fn cover_art(&self, song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        match self.client.get_cover_art(song_id, None).await {
            Ok(bytes) if !bytes.is_empty() => Ok(Some(bytes.to_vec())),
            Ok(_) => Ok(None),
            Err(e) => {
                tracing::debug!(
                    "subsonic source '{}': no cover art for {} ({})",
                    self.name,
                    song_id,
                    map_err(e)
                );
                Ok(None)
            }
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "subsonic"))]
mod tests {
    use super::*;
    use rmpd_core::config::SourceConfig;

    /// Construct a minimal `SubsonicSource` for unit testing (no network).
    fn make_source(name: &str) -> SubsonicSource {
        let client = Client::new("http://localhost", Auth::api_key("test-key"))
            .expect("http://localhost should parse");
        SubsonicSource {
            name: name.to_string(),
            client,
            max_bitrate: None,
            format: None,
            layout: Layout::Tags,
        }
    }

    /// Construct a representative `Child` via serde_json so we don't have to
    /// enumerate all ~40 optional fields as struct literal None-fields.
    fn make_child() -> opensubsonic::data::Child {
        serde_json::from_value(serde_json::json!({
            "id": "song-123",
            "suffix": "flac",
            "isDir": false,
            "title": "Test Song",
            "album": "Test Album",
            "artist": "Test Artist",
            "track": 3,
            "year": 2023,
            "genre": "Rock",
            "duration": 240,
            "bitRate": 320,
            "bitDepth": 24,
            "samplingRate": 48000,
            "channelCount": 2,
            "discNumber": 1
        }))
        .expect("valid Child JSON")
    }

    /// When the server omits `suffix` (some servers only set it on getSong),
    /// the path extension is derived from the MIME `content_type` so clients
    /// still see the codec.
    #[test]
    fn map_song_derives_extension_from_content_type() {
        let source = make_source("home");
        let child: opensubsonic::data::Child = serde_json::from_value(serde_json::json!({
            "id": "abc",
            "isDir": false,
            "title": "No Suffix",
            "artist": "A",
            "album": "B",
            "contentType": "audio/flac"
        }))
        .expect("valid Child JSON");
        let song = source.map_song(&child);
        assert_eq!(song.path.as_str(), "home/A/B/abc.flac");
    }

    #[test]
    fn mime_to_ext_maps_common_types() {
        assert_eq!(mime_to_ext("audio/mpeg"), Some("mp3"));
        assert_eq!(mime_to_ext("audio/flac"), Some("flac"));
        assert_eq!(mime_to_ext("audio/mp4; codecs=\"mp4a.40.2\""), Some("m4a"));
        assert_eq!(mime_to_ext("audio/unknown"), None);
    }

    // ── (a) map_song tags and audio properties ────────────────────────────────

    #[test]
    fn map_song_path_tags_and_audio_props() {
        let source = make_source("home");
        let child = make_child();
        let song = source.map_song(&child);

        // Virtual path
        assert_eq!(
            song.path.as_str(),
            "home/Test Artist/Test Album/song-123.flac"
        );

        let tag = |name: &str| -> Option<&str> {
            song.tags
                .iter()
                .find(|(k, _)| k.as_ref() == name)
                .map(|(_, v)| v.as_str())
        };

        assert_eq!(tag("title"), Some("Test Song"));
        assert_eq!(tag("artist"), Some("Test Artist"));
        assert_eq!(tag("albumartist"), Some("Test Artist"));
        assert_eq!(tag("album"), Some("Test Album"));
        assert_eq!(tag("track"), Some("3"));
        assert_eq!(tag("date"), Some("2023"));
        assert_eq!(tag("genre"), Some("Rock"));
        assert_eq!(tag("disc"), Some("1"));

        // Audio properties
        assert_eq!(song.duration, Some(std::time::Duration::from_secs(240)));
        assert_eq!(song.sample_rate, Some(48_000));
        assert_eq!(song.channels, Some(2));
        assert_eq!(song.bits_per_sample, Some(24));
        assert_eq!(song.bitrate, Some(320));
    }

    // ── (b) Path round-trip ───────────────────────────────────────────────────

    /// The trailing `/`-segment of the mount-style path carries the raw
    /// `child.id` plus the lower-cased file extension; stripping the extension
    /// (what `SourceRegistry::resolve_stream_uri` does) recovers the bare id.
    #[test]
    fn path_trailing_segment_is_child_id_with_extension() {
        let source = make_source("myserver");
        let child = make_child();
        let song = source.map_song(&child);

        let path_str = song.path.as_str();
        let trailing = path_str.rsplit('/').next().expect("path has segments");
        assert_eq!(trailing, "song-123.flac");
        // Stripping the audio extension round-trips to the original id.
        let stem = trailing
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(trailing);
        assert_eq!(stem, child.id.as_str());
    }

    /// Artist/album names containing `%` or `/` are encoded so they cannot
    /// corrupt the id extraction, while the id itself is left unencoded.
    #[test]
    fn special_chars_in_artist_album_are_encoded() {
        let source = make_source("home");
        let child: opensubsonic::data::Child = serde_json::from_value(serde_json::json!({
            "id": "abc123",
            "isDir": false,
            "title": "Edge Case",
            "artist": "Artist/With/Slashes",
            "album": "100% Real Album"
        }))
        .unwrap();

        let song = source.map_song(&child);
        let path_str = song.path.as_str();

        // Mount-style: starts with the source name, no scheme prefix.
        assert!(path_str.starts_with("home/"));
        assert!(!path_str.contains("://"));

        // id is NOT encoded (no suffix on this child, so the leaf is the bare id).
        let trailing = path_str.rsplit('/').next().unwrap();
        assert_eq!(trailing, "abc123");

        // The encoded artist/album segments must not contain raw `/` or `%`
        // (only the segment separators are raw slashes).
        assert!(path_str.contains("Artist%2FWith%2FSlashes"));
        assert!(path_str.contains("100%25 Real Album"));
    }

    // ── (c) Config validation ─────────────────────────────────────────────────

    fn make_cfg_with(settings: toml::Table) -> SourceConfig {
        SourceConfig {
            name: "test".to_owned(),
            source_type: "subsonic".to_owned(),
            enabled: true,
            settings,
        }
    }

    #[test]
    fn from_source_config_missing_auth_returns_config_error() {
        let mut settings = toml::Table::new();
        settings.insert(
            "url".to_owned(),
            toml::Value::String("http://music.example.com".to_owned()),
        );
        // No api_key, no username, no password.
        let cfg = make_cfg_with(settings);
        let result = SubsonicConfig::from_source_config(&cfg);
        assert!(
            matches!(result, Err(SourceError::Config(_))),
            "expected Config error, got {result:?}"
        );
    }

    #[test]
    fn from_source_config_missing_url_returns_config_error() {
        let mut settings = toml::Table::new();
        settings.insert(
            "api_key".to_owned(),
            toml::Value::String("my-key".to_owned()),
        );
        let cfg = make_cfg_with(settings);
        let result = SubsonicConfig::from_source_config(&cfg);
        assert!(
            matches!(result, Err(SourceError::Config(_))),
            "expected Config error, got {result:?}"
        );
    }

    #[test]
    fn from_source_config_api_key_ok() {
        let mut settings = toml::Table::new();
        settings.insert(
            "url".to_owned(),
            toml::Value::String("http://music.example.com".to_owned()),
        );
        settings.insert(
            "api_key".to_owned(),
            toml::Value::String("secret-key".to_owned()),
        );
        let cfg = make_cfg_with(settings);
        let result = SubsonicConfig::from_source_config(&cfg);
        assert!(result.is_ok(), "api_key auth should be accepted");
    }

    #[test]
    fn from_source_config_username_password_ok() {
        let mut settings = toml::Table::new();
        settings.insert(
            "url".to_owned(),
            toml::Value::String("http://music.example.com".to_owned()),
        );
        settings.insert(
            "username".to_owned(),
            toml::Value::String("admin".to_owned()),
        );
        settings.insert(
            "password".to_owned(),
            toml::Value::String("secret".to_owned()),
        );
        let cfg = make_cfg_with(settings);
        let result = SubsonicConfig::from_source_config(&cfg);
        assert!(result.is_ok(), "username+password auth should be accepted");
    }

    #[test]
    fn from_source_config_only_username_returns_config_error() {
        let mut settings = toml::Table::new();
        settings.insert(
            "url".to_owned(),
            toml::Value::String("http://music.example.com".to_owned()),
        );
        settings.insert(
            "username".to_owned(),
            toml::Value::String("admin".to_owned()),
        );
        // No password, no api_key.
        let cfg = make_cfg_with(settings);
        let result = SubsonicConfig::from_source_config(&cfg);
        assert!(
            matches!(result, Err(SourceError::Config(_))),
            "username without password should fail"
        );
    }

    // ── enc helper ────────────────────────────────────────────────────────────

    #[test]
    fn enc_encodes_percent_and_slash() {
        assert_eq!(enc("hello"), "hello");
        assert_eq!(enc("AC/DC"), "AC%2FDC");
        assert_eq!(enc("100%"), "100%25");
        assert_eq!(enc("a/b%c"), "a%2Fb%25c");
    }

    // ── map_err ───────────────────────────────────────────────────────────────

    #[test]
    fn map_err_auth_codes() {
        for code in [40i32, 41, 42, 43, 44] {
            let e = SubsonicError::Api(opensubsonic::SubsonicApiError {
                code,
                message: "auth failure".to_owned(),
                help_url: None,
            });
            assert!(
                matches!(map_err(e), SourceError::Auth(_)),
                "code {code} should map to Auth"
            );
        }
    }

    #[test]
    fn map_err_non_auth_api_code_is_protocol() {
        let e = SubsonicError::Api(opensubsonic::SubsonicApiError {
            code: 70,
            message: "not found".to_owned(),
            help_url: None,
        });
        assert!(matches!(map_err(e), SourceError::Protocol(_)));
    }

    #[test]
    fn map_err_parse_is_protocol() {
        let e = SubsonicError::Parse("bad json".to_owned());
        assert!(matches!(map_err(e), SourceError::Protocol(_)));
    }

    // ── Lazy browse fixtures ────────────────────────────────────────────────

    fn artists_fixture() -> opensubsonic::data::ArtistsId3 {
        serde_json::from_value(serde_json::json!({
            "index": [
                {"name": "A", "artist": [
                    {"id": "ar1", "name": "AC/DC"},
                    {"id": "ar2", "name": "Air"}
                ]},
                {"name": "P", "artist": [{"id": "ar3", "name": "Portishead"}]}
            ]
        }))
        .expect("valid artists JSON")
    }

    fn albums_fixture() -> Vec<opensubsonic::data::AlbumId3> {
        serde_json::from_value(serde_json::json!([
            {"id": "al1", "name": "Back/In Black"},
            {"id": "al2", "name": "Highway"},
            {"id": "al3", "name": "Highway"}
        ]))
        .expect("valid albums JSON")
    }

    fn dirs(entries: Vec<SourceEntry>) -> Vec<String> {
        entries
            .into_iter()
            .filter_map(|e| match e {
                SourceEntry::Dir(d) => Some(d),
                SourceEntry::Song(_) => None,
            })
            .collect()
    }

    #[test]
    fn dec_inverts_enc_and_tolerates_malformed() {
        assert_eq!(dec(&enc("AC/DC 100%")), "AC/DC 100%");
        assert_eq!(dec("a%2"), "a%2");
        assert_eq!(dec("%zz"), "%zz");
        assert_eq!(dir_segments("/AC%2FDC//Back/"), vec!["AC/DC", "Back"]);
        assert!(dir_segments("").is_empty());
    }

    #[test]
    fn artist_entries_are_encoded_virtual_dirs() {
        let src = make_source("home");
        let d = dirs(src.artist_entries(&artists_fixture()));
        assert_eq!(d, vec!["home/AC%2FDC", "home/Air", "home/Portishead"]);
    }

    #[test]
    fn find_artist_id_matches_decoded_name() {
        let a = artists_fixture();
        assert_eq!(find_artist_id(&a, "AC/DC"), Some("ar1"));
        assert_eq!(find_artist_id(&a, "Nobody"), None);
    }

    #[test]
    fn album_entries_and_duplicate_album_ids() {
        let src = make_source("home");
        let albums = albums_fixture();
        let d = dirs(src.album_entries("AC/DC", &albums));
        assert_eq!(d[0], "home/AC%2FDC/Back%2FIn Black");
        assert_eq!(matching_album_ids(&albums, "Highway"), vec!["al2", "al3"]);
        assert!(matching_album_ids(&albums, "Nope").is_empty());
    }

    #[test]
    fn browsed_album_songs_match_synced_paths() {
        let src = make_source("home");
        let album: opensubsonic::data::AlbumWithSongsId3 =
            serde_json::from_value(serde_json::json!({
                "id": "al1",
                "name": "Dummy",
                "artist": "Portishead",
                "song": [
                    {"id": "s1", "title": "Roads", "suffix": "flac", "isDir": false},
                    {"id": "s2", "title": "Glory Box", "isDir": false}
                ]
            }))
            .expect("valid album JSON");
        let entries = src.album_song_entries(&album);
        let paths: Vec<String> = entries
            .into_iter()
            .filter_map(|e| match e {
                SourceEntry::Song(s) => Some(s.path.to_string()),
                SourceEntry::Dir(_) => None,
            })
            .collect();
        // Same grammar and fallbacks as `list_all` (`map_album_song`).
        assert_eq!(
            paths,
            vec!["home/Portishead/Dummy/s1.flac", "home/Portishead/Dummy/s2"]
        );
    }

    // ── Layout setting ────────────────────────────────────────────────────

    fn layout_cfg(layout: Option<&str>) -> Result<SubsonicConfig, SourceError> {
        let mut settings = toml::Table::new();
        settings.insert("url".to_owned(), "http://music.example.com".into());
        settings.insert("api_key".to_owned(), "k".into());
        if let Some(l) = layout {
            settings.insert("layout".to_owned(), l.into());
        }
        SubsonicConfig::from_source_config(&make_cfg_with(settings))
    }

    #[test]
    fn layout_defaults_to_tags_and_parses_case_insensitively() {
        assert_eq!(layout_cfg(None).unwrap().layout, Layout::Tags);
        assert_eq!(layout_cfg(Some("tags")).unwrap().layout, Layout::Tags);
        assert_eq!(layout_cfg(Some("folders")).unwrap().layout, Layout::Folders);
        assert_eq!(
            layout_cfg(Some(" Folders ")).unwrap().layout,
            Layout::Folders
        );
    }

    #[test]
    fn unknown_layout_is_a_config_error() {
        for bad in ["path", "folder", "both"] {
            assert!(
                matches!(layout_cfg(Some(bad)), Err(SourceError::Config(_))),
                "layout = {bad:?} must be rejected"
            );
        }
        assert!(SETTINGS.contains(&"layout"));
    }

    // ── Folder layout mapping ─────────────────────────────────────────────

    fn folder_source() -> SubsonicSource {
        let mut s = make_source("home");
        s.layout = Layout::Folders;
        s
    }

    fn listing(children: serde_json::Value) -> Vec<opensubsonic::data::Child> {
        serde_json::from_value(children).expect("valid children JSON")
    }

    #[test]
    fn folder_seg_escapes_what_a_path_cannot_hold() {
        assert_eq!(folder_seg("Rock"), "Rock");
        assert_eq!(folder_seg("AC/DC"), "AC%2FDC");
        assert_eq!(folder_seg(""), "_");
        assert_eq!(folder_seg("."), "%2E");
        assert_eq!(folder_seg(".."), "%2E%2E");
        assert_eq!(dir_segments("%2E%2E/a"), vec!["..", "a"]);
    }

    #[test]
    fn split_children_maps_dirs_and_songs_below_the_directory_path() {
        let src = folder_source();
        let kids = listing(serde_json::json!([
            {"id": "d1", "isDir": true, "title": "CD1/Bonus"},
            {"id": "d2", "isDir": true, "title": ".."},
            {"id": "s1", "isDir": false, "title": "Roads", "suffix": "FLAC",
             "artist": "Portishead", "album": "Dummy"},
            {"id": "s2", "isDir": false, "title": "No Suffix"},
            {"id": "v1", "isDir": false, "title": "Clip", "isVideo": true}
        ]));
        let level = src.split_children("home/Trip-Hop/Dummy", &kids);

        assert_eq!(
            level.dirs,
            vec![
                (
                    "home/Trip-Hop/Dummy/CD1%2FBonus".to_owned(),
                    "d1".to_owned()
                ),
                ("home/Trip-Hop/Dummy/%2E%2E".to_owned(), "d2".to_owned()),
            ]
        );
        let paths: Vec<&str> = level.songs.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["home/Trip-Hop/Dummy/s1.flac", "home/Trip-Hop/Dummy/s2"],
            "video skipped, leaf is <id>[.<ext>]"
        );
    }

    /// The stream/cover-art id must survive the folder path exactly as in the
    /// tag layout, whatever the directory names contain.
    #[test]
    fn folder_song_leaf_round_trips_the_id() {
        let src = folder_source();
        let kids = listing(serde_json::json!([
            {"id": "tr-0.9", "isDir": false, "title": "T", "suffix": "mp3"},
            {"id": "al.flac", "isDir": false, "title": "U"}
        ]));
        let level = src.split_children("home/100%25 Hits/a.flac", &kids);
        let ids: Vec<&str> = level
            .songs
            .iter()
            .map(|s| crate::extract_remote_id(s.path.as_str()))
            .collect();
        // `tr-0.9` keeps its dot (unknown extension); `al.flac` is an id that
        // happens to look like a file name, exactly as in the tag layout.
        assert_eq!(ids, vec!["tr-0.9", "al"]);
        assert!(
            level.songs[0]
                .path
                .as_str()
                .starts_with("home/100%25 Hits/")
        );
    }

    #[test]
    fn folder_songs_keep_server_tags_without_placeholders() {
        let src = folder_source();
        let kids = listing(serde_json::json!([
            {"id": "s1", "isDir": false, "title": "Bare", "track": 2, "year": 1994}
        ]));
        let song = &src.split_children("home/x", &kids).songs[0];
        let keys: Vec<&str> = song.tags.iter().map(|(k, _)| k.as_ref()).collect();
        assert_eq!(keys, vec!["title", "track", "date"]);
    }

    #[test]
    fn split_indexes_yields_root_dirs_and_loose_children() {
        let src = folder_source();
        let indexes: opensubsonic::data::Indexes = serde_json::from_value(serde_json::json!({
            "index": [
                {"name": "A", "artist": [{"id": "f1", "name": "Ambient"}]},
                {"name": "R", "artist": [{"id": "f2", "name": "Rock/Pop"}]}
            ],
            "shortcut": [{"id": "f1", "name": "Ambient"}],
            "child": [
                {"id": "s0", "isDir": false, "title": "Loose", "suffix": "mp3"},
                {"id": "f3", "isDir": true, "title": "Misc"}
            ]
        }))
        .expect("valid indexes JSON");
        let level = src.split_indexes(&indexes);

        let dirs: Vec<(&str, &str)> = level
            .dirs
            .iter()
            .map(|(p, id)| (p.as_str(), id.as_str()))
            .collect();
        assert_eq!(
            dirs,
            vec![
                ("home/Misc", "f3"),
                ("home/Ambient", "f1"),
                ("home/Rock%2FPop", "f2")
            ],
            "shortcuts must not duplicate indexed dirs"
        );
        assert_eq!(level.songs.len(), 1);
        assert_eq!(level.songs[0].path.as_str(), "home/s0.mp3");
    }

    #[test]
    fn folder_level_entries_list_dirs_then_songs() {
        let src = folder_source();
        let kids = listing(serde_json::json!([
            {"id": "s1", "isDir": false, "title": "One"},
            {"id": "d1", "isDir": true, "title": "Sub"}
        ]));
        let entries = src.split_children("home/x", &kids).into_entries();
        assert!(matches!(&entries[0], SourceEntry::Dir(d) if d == "home/x/Sub"));
        assert!(matches!(&entries[1], SourceEntry::Song(s) if s.path.as_str() == "home/x/s1"));
    }

    /// Switching `layout` and resyncing leaves only the new tree behind: the
    /// mapped paths of both layouts go through the real database path.
    #[test]
    fn layout_switch_resync_leaves_no_stale_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("t.db");
        let db = rmpd_library::Database::open(db_path.to_str().expect("utf8")).expect("open");
        let token = "subsonic:home";

        let child: opensubsonic::data::Child = serde_json::from_value(serde_json::json!({
            "id": "s1", "isDir": false, "title": "Roads", "suffix": "flac",
            "artist": "AC/DC", "album": "Back In Black"
        }))
        .expect("valid child");

        let tags = make_source("home").map_song(&child);
        assert_eq!(tags.path.as_str(), "home/AC%2FDC/Back In Black/s1.flac");
        db.add_source_song(&tags, token).expect("insert tags song");

        // Resync as folders: same song, now under its server directory.
        db.with_transaction(|db| {
            db.clear_source(token)?;
            let folders = folder_source().map_folder_song(&child, "home/Rock/90s");
            db.add_source_song(&folders, token)?;
            db.prune_empty_source_directories("home")?;
            Ok(())
        })
        .expect("resync");

        let mount = db.list_directory("home").expect("mount");
        let subdirs: Vec<&str> = mount.directories.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(subdirs, vec!["home/Rock"]);
        assert!(db.list_directory("home/AC%2FDC").is_err());
        let leaf = db.list_directory("home/Rock/90s").expect("folder dir");
        assert_eq!(leaf.songs[0].path.as_str(), "home/Rock/90s/s1.flac");

        // And back to tags: the folder chain disappears again.
        db.with_transaction(|db| {
            db.clear_source(token)?;
            db.add_source_song(&tags, token)?;
            db.prune_empty_source_directories("home")?;
            Ok(())
        })
        .expect("resync back");
        assert!(db.list_directory("home/Rock").is_err());
        assert_eq!(
            db.list_directory("home/AC%2FDC/Back In Black")
                .expect("tag dir")
                .songs
                .len(),
            1
        );
    }

    // ── Folder walk against a mock server ─────────────────────────────────

    /// Minimal HTTP/1.1 server answering the three folder endpoints from a
    /// fixed tree. Returns the base URL and the number of
    /// `getMusicDirectory` requests served per id.
    async fn spawn_folder_server() -> (
        String,
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let hits = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let served = std::sync::Arc::clone(&hits);

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let served = std::sync::Arc::clone(&served);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let target = req.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let id = target
                        .split(['?', '&'])
                        .find_map(|kv| kv.strip_prefix("id="))
                        .unwrap_or("")
                        .to_owned();
                    let body = if target.contains("getIndexes") {
                        serde_json::json!({"indexes": {
                            "index": [{"name": "A", "artist": [{"id": "f1", "name": "Albums"}]}],
                            "child": [{"id": "s0", "isDir": false, "title": "Loose", "suffix": "mp3"}]
                        }})
                    } else if target.contains("getMusicDirectory") {
                        *served.lock().expect("lock").entry(id.clone()).or_insert(0) += 1;
                        match id.as_str() {
                            "f1" => serde_json::json!({"directory": {
                            "id": "f1", "name": "Albums", "child": [
                                {"id": "f2", "isDir": true, "title": "Dummy"},
                                {"id": "f3", "isDir": true, "title": "Broken"},
                                // Same directory linked a second time.
                                {"id": "f2", "isDir": true, "title": "Dummy again"},
                                {"id": "s1", "isDir": false, "title": "One", "suffix": "flac"}
                            ]}}),
                            "f2" => serde_json::json!({"directory": {
                            "id": "f2", "name": "Dummy", "child": [
                                {"id": "s2", "isDir": false, "title": "Two", "suffix": "ogg"},
                                {"id": "s3", "isDir": false, "title": "Three"}
                            ]}}),
                            _ => serde_json::json!({
                                "status": "failed",
                                "error": {"code": 70, "message": "not found"}
                            }),
                        }
                    } else {
                        serde_json::json!({})
                    };
                    let mut resp = serde_json::json!({"status": "ok", "version": "1.16.1"});
                    for (k, v) in body.as_object().expect("object") {
                        resp[k] = v.clone();
                    }
                    let payload = serde_json::json!({ "subsonic-response": resp }).to_string();
                    let http = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = sock.write_all(http.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (url, hits)
    }

    fn folder_source_at(url: &str) -> SubsonicSource {
        SubsonicSource {
            name: "home".to_owned(),
            client: Client::new(url, Auth::api_key("k")).expect("url"),
            max_bitrate: None,
            format: None,
            layout: Layout::Folders,
        }
    }

    #[tokio::test]
    async fn list_all_in_folder_layout_walks_the_server_tree() {
        let (url, hits) = spawn_folder_server().await;
        let src = folder_source_at(&url);

        let mut paths: Vec<String> = src
            .list_all()
            .await
            .expect("walk")
            .into_iter()
            .map(|s| s.path.to_string())
            .collect();
        paths.sort();
        // `Broken` fails (skipped, not fatal); the doubly linked `f2` is
        // fetched once, so its tracks appear once.
        assert_eq!(
            paths,
            vec![
                "home/Albums/Dummy/s2.ogg",
                "home/Albums/Dummy/s3",
                "home/Albums/s1.flac",
                "home/s0.mp3",
            ]
        );
        let hits = hits.lock().expect("lock");
        assert_eq!(hits.get("f2"), Some(&1), "directory ids are visited once");
        assert_eq!(hits.get("f3"), Some(&1));
    }

    #[tokio::test]
    async fn browse_in_folder_layout_lists_one_server_level() {
        let (url, _) = spawn_folder_server().await;
        let src = folder_source_at(&url);

        let names = |entries: Vec<SourceEntry>| -> Vec<String> {
            entries
                .into_iter()
                .map(|e| match e {
                    SourceEntry::Dir(d) => format!("dir:{d}"),
                    SourceEntry::Song(s) => format!("song:{}", s.path),
                })
                .collect()
        };

        assert_eq!(
            names(src.browse("").await.expect("root")),
            vec!["dir:home/Albums", "song:home/s0.mp3"]
        );
        assert_eq!(
            names(src.browse("Albums/Dummy").await.expect("leaf")),
            vec!["song:home/Albums/Dummy/s2.ogg", "song:home/Albums/Dummy/s3"]
        );
        assert!(matches!(
            src.browse("Albums/Nope").await,
            Err(SourceError::NotFound(_))
        ));
    }
}
