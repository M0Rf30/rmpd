//! `SubsonicSource` — OpenSubsonic/Subsonic REST API backend.
//!
//! Compiled only when `feature = "subsonic"` is active (declared in `lib.rs`).
//! No network I/O occurs at construction time; the client is validated lazily on `ping`.

use async_trait::async_trait;
use camino::Utf8PathBuf;
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
            .finish()
    }
}

impl SubsonicConfig {
    /// Parse and validate settings from a `[[source]]` config block.
    ///
    /// # Errors
    /// - `url` is absent → `SourceError::Config`
    /// - Neither `api_key` nor (`username` + `password`) is present → `SourceError::Config`
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

        Ok(Self {
            name: cfg.name.clone(),
            url,
            username,
            password,
            api_key,
            max_bitrate,
            format,
            accept_invalid_certs,
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
}

// ─── Factory ─────────────────────────────────────────────────────────────────

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
        // Validated above: both are Some.
        Auth::token(sc.username.unwrap(), sc.password.unwrap())
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
    }))
}

// ─── Song mapping ────────────────────────────────────────────────────────────

impl SubsonicSource {
    /// Convert a Subsonic `Child` to an `rmpd_core::song::Song` with a virtual path.
    ///
    /// Virtual path scheme:
    /// ```text
    /// subsonic://<source-name>/<enc(artist)>/<enc(album)>/<child.id>
    /// ```
    /// `enc()` percent-encodes `%` and `/` so that splitting on `/` and taking
    /// the trailing segment always recovers the original unencoded `child.id`.
    fn map_song(&self, c: &opensubsonic::data::Child) -> Song {
        let artist = c.artist.as_deref().unwrap_or("Unknown Artist");
        let album = c.album.as_deref().unwrap_or("Unknown Album");

        // child.id is NOT encoded — Subsonic IDs are URL-safe tokens.
        let path = Utf8PathBuf::from(format!(
            "subsonic://{}/{}/{}/{}",
            self.name,
            enc(artist),
            enc(album),
            c.id,
        ));

        let mut tags: Vec<(std::borrow::Cow<'static, str>, String)> = Vec::new();

        // title is always present on Child
        tags.push((intern_tag_key("title"), c.title.clone()));

        if let Some(a) = &c.artist {
            tags.push((intern_tag_key("artist"), a.clone()));
            // albumartist defaults to artist when no dedicated field is available
            tags.push((intern_tag_key("albumartist"), a.clone()));
        }
        if let Some(alb) = &c.album {
            tags.push((intern_tag_key("album"), alb.clone()));
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
            tags,
        }
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

    /// Browse the remote library.
    ///
    /// When `dir` is empty (root), returns one `SourceEntry::Dir` per artist.
    /// Deeper levels return an empty list — real browsing is DB-backed via `lsinfo`
    /// after a catalog sync.
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        if dir.is_empty() || dir == "/" {
            let artists = self.client.get_artists(None).await.map_err(map_err)?;
            let entries = artists
                .index
                .iter()
                .flat_map(|idx| idx.artist.iter())
                .map(|a| SourceEntry::Dir(format!("subsonic://{}/{}", self.name, enc(&a.name))))
                .collect();
            Ok(entries)
        } else {
            // TODO: lazy deep browse; DB-backed lsinfo is the real path
            Ok(vec![])
        }
    }

    /// Walk the full artist → album → song tree and return every song.
    ///
    /// Sequential awaits are used deliberately for v1; the catalog-sync path
    /// that calls this is already running in a background task.
    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        let artists = self.client.get_artists(None).await.map_err(map_err)?;
        let mut songs = Vec::new();

        for index in &artists.index {
            for artist_stub in &index.artist {
                let artist = self
                    .client
                    .get_artist(&artist_stub.id)
                    .await
                    .map_err(map_err)?;

                for album_stub in &artist.album {
                    let album = self
                        .client
                        .get_album(&album_stub.id)
                        .await
                        .map_err(map_err)?;

                    for child in &album.song {
                        songs.push(self.map_song(child));
                    }
                }
            }
        }

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
        }
    }

    /// Construct a representative `Child` via serde_json so we don't have to
    /// enumerate all ~40 optional fields as struct literal None-fields.
    fn make_child() -> opensubsonic::data::Child {
        serde_json::from_value(serde_json::json!({
            "id": "song-123",
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

    // ── (a) map_song tags and audio properties ────────────────────────────────

    #[test]
    fn map_song_path_tags_and_audio_props() {
        let source = make_source("home");
        let child = make_child();
        let song = source.map_song(&child);

        // Virtual path
        assert_eq!(
            song.path.as_str(),
            "subsonic://home/Test Artist/Test Album/song-123"
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

    /// The trailing `/`-segment of the virtual path must equal the original
    /// `child.id`, matching what `SourceRegistry::resolve_stream_uri` extracts.
    #[test]
    fn path_trailing_segment_is_child_id() {
        let source = make_source("myserver");
        let child = make_child();
        let song = source.map_song(&child);

        let path_str = song.path.as_str();
        let trailing = path_str.rsplit('/').next().expect("path has segments");
        assert_eq!(trailing, child.id.as_str());
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

        // id is NOT encoded
        let trailing = path_str.rsplit('/').next().unwrap();
        assert_eq!(trailing, "abc123");

        // The encoded artist/album segments must not contain raw `/` or `%`
        // (only the final `/<id>` separator is a raw slash).
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
}
