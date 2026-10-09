// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `JellyfinSource` — Jellyfin (and Emby-compatible) music backend.
//!
//! Compiled only when `feature = "jellyfin"` is active. No network I/O occurs
//! at construction time; authentication happens lazily on first use and the
//! session (access token + user id) is cached for the life of the source.
//!
//! * Catalog: `SyncPolicy::Full` — `list_all` pages through
//!   `/Items?IncludeItemTypes=Audio&Recursive=true`.
//! * Streaming: `/Audio/{id}/stream?static=true` (original file), or
//!   `/Audio/{id}/universal` when `max_bitrate` / `format` request transcoding.
//! * Cover art: `/Items/{id}/Images/Primary` (falls back to the album image).
//! * Playlists: `/Items?IncludeItemTypes=Playlist` + `/Playlists/{id}/Items`.

use crate::common::{
    dir_segments, enc, http_client, known_ext, leaf_id, make_song, normalize_base_url, send_bytes,
    setting_bool, urlenc,
};
use async_trait::async_trait;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Setting keys accepted in a `[[source]] type = "jellyfin"` block.
pub const SETTINGS: &[&str] = &[
    "url",
    "username",
    "password",
    "api_key",
    "user_id",
    "max_bitrate",
    "format",
    "accept_invalid_certs",
];

/// Items requested per `/Items` page.
const PAGE: usize = 500;

/// Fields requested alongside each audio item.
const ITEM_FIELDS: &str = "Genres,MediaSources,ProductionYear";

/// Container list advertised to `/universal` (direct-play candidates).
const DIRECT_CONTAINERS: &str = "flac,mp3,ogg,opus,m4a,aac,wav";

// ─── Wire types ──────────────────────────────────────────────────────────────

fn vec_or_null<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// `{"Items":[...],"TotalRecordCount":N}` envelope.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct ItemsResponse {
    #[serde(deserialize_with = "vec_or_null")]
    items: Vec<BaseItem>,
}

/// The subset of Jellyfin's `BaseItemDto` rmpd cares about.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct BaseItem {
    id: String,
    name: Option<String>,
    album: Option<String>,
    album_id: Option<String>,
    album_artist: Option<String>,
    #[serde(deserialize_with = "vec_or_null")]
    artists: Vec<String>,
    /// Duration in 100 ns ticks.
    run_time_ticks: Option<u64>,
    index_number: Option<u32>,
    parent_index_number: Option<u32>,
    production_year: Option<u32>,
    #[serde(deserialize_with = "vec_or_null")]
    genres: Vec<String>,
    container: Option<String>,
    #[serde(deserialize_with = "vec_or_null")]
    media_sources: Vec<MediaSource>,
    #[serde(rename = "Type")]
    item_type: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaSource {
    container: Option<String>,
    /// Bits per second.
    bitrate: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct AuthResult {
    access_token: String,
    user: AuthUser,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct AuthUser {
    id: String,
}

fn parse_items(body: &[u8]) -> SourceResult<Vec<BaseItem>> {
    serde_json::from_slice::<ItemsResponse>(body)
        .map(|r| r.items)
        .map_err(|e| SourceError::Protocol(format!("invalid Jellyfin response: {e}")))
}

// ─── Mapping ─────────────────────────────────────────────────────────────────

/// Convert a Jellyfin audio item to a `Song` with a mount-style virtual path
/// `<source>/<enc(artist)>/<enc(album)>/<id>[.<ext>]`.
fn map_item(source: &str, it: &BaseItem) -> Song {
    let artist_ref = it
        .artists
        .first()
        .map(String::as_str)
        .or(it.album_artist.as_deref());
    let artist = artist_ref.unwrap_or("Unknown Artist");
    let album = it.album.as_deref().unwrap_or("Unknown Album");

    let src0 = it.media_sources.first();
    let ext = it.container.as_deref().and_then(known_ext).or_else(|| {
        src0.and_then(|m| m.container.as_deref())
            .and_then(known_ext)
    });
    let leaf = match ext {
        Some(e) => format!("{}.{}", it.id, e),
        None => it.id.clone(),
    };
    let path = format!("{}/{}/{}/{}", source, enc(artist), enc(album), leaf);

    let mut tags: Vec<(&str, String)> = vec![("title", it.name.clone().unwrap_or_default())];
    if it.artists.is_empty() {
        if let Some(a) = it.album_artist.as_deref() {
            tags.push(("artist", a.to_owned()));
        }
    } else {
        for a in &it.artists {
            tags.push(("artist", a.clone()));
        }
    }
    if let Some(aa) = it.album_artist.as_deref().or(artist_ref) {
        tags.push(("albumartist", aa.to_owned()));
    }
    if let Some(a) = it.album.as_deref() {
        tags.push(("album", a.to_owned()));
    }
    if let Some(n) = it.index_number {
        tags.push(("track", n.to_string()));
    }
    if let Some(n) = it.parent_index_number {
        tags.push(("disc", n.to_string()));
    }
    if let Some(y) = it.production_year {
        tags.push(("date", y.to_string()));
    }
    for g in &it.genres {
        tags.push(("genre", g.clone()));
    }

    let duration = it
        .run_time_ticks
        .map(|t| Duration::from_micros(t / 10))
        .filter(|d| !d.is_zero());
    let bitrate = src0
        .and_then(|m| m.bitrate)
        .and_then(|b| u32::try_from(b / 1000).ok())
        .filter(|b| *b > 0);
    make_song(path, tags, duration, bitrate)
}

/// `X-Emby-Authorization` / `Authorization` header value.
fn auth_header(device_id: &str, token: Option<&str>) -> String {
    let mut v = format!(
        "MediaBrowser Client=\"rmpd\", Device=\"rmpd\", DeviceId=\"{}\", Version=\"{}\"",
        device_id.replace('"', ""),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(t) = token {
        v.push_str(&format!(", Token=\"{}\"", t.replace('"', "")));
    }
    v
}

/// Build the playable URL for `id` (see module docs). Contains the access
/// token as `api_key`, which the HTTP decoder needs.
fn build_stream_url(
    base: &str,
    id: &str,
    token: &str,
    user_id: &str,
    device_id: &str,
    max_bitrate_kbps: Option<u32>,
    format: Option<&str>,
) -> String {
    let id_enc = urlenc(id);
    if max_bitrate_kbps.is_none() && format.is_none() {
        return format!(
            "{base}/Audio/{id_enc}/stream?static=true&api_key={}",
            urlenc(token)
        );
    }
    let codec = format.map_or_else(|| "mp3".to_owned(), str::to_ascii_lowercase);
    let container = format.map_or_else(
        || DIRECT_CONTAINERS.to_owned(),
        |f| urlenc(&f.to_ascii_lowercase()),
    );
    let mut url = format!(
        "{base}/Audio/{id_enc}/universal?UserId={}&DeviceId={}&api_key={}&Container={}\
         &TranscodingContainer={}&TranscodingProtocol=http&AudioCodec={}",
        urlenc(user_id),
        urlenc(device_id),
        urlenc(token),
        container,
        urlenc(&codec),
        urlenc(&codec),
    );
    if let Some(b) = max_bitrate_kbps {
        url.push_str(&format!("&MaxStreamingBitrate={}", u64::from(b) * 1000));
    }
    url
}

// ─── Config ──────────────────────────────────────────────────────────────────

/// Validated view of a `[[source]]` block for the Jellyfin backend.
pub struct JellyfinConfig {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_key: Option<String>,
    pub user_id: Option<String>,
    pub max_bitrate: Option<u32>,
    pub format: Option<String>,
    pub accept_invalid_certs: bool,
}

/// Redacts credentials so a `{:?}` can never leak secrets.
impl std::fmt::Debug for JellyfinConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JellyfinConfig")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("user_id", &self.user_id)
            .field("max_bitrate", &self.max_bitrate)
            .field("format", &self.format)
            .field("accept_invalid_certs", &self.accept_invalid_certs)
            .finish()
    }
}

impl JellyfinConfig {
    /// Parse and validate settings.
    ///
    /// # Errors
    /// `url` absent/invalid, or neither `api_key` nor `username`+`password`.
    pub fn from_source_config(cfg: &SourceConfig) -> Result<Self, SourceError> {
        let raw = cfg.setting_str("url").ok_or_else(|| {
            SourceError::Config("jellyfin source requires a `url` setting".to_owned())
        })?;
        let url = normalize_base_url(&raw, "jellyfin")?;
        let api_key = cfg.setting_str("api_key");
        let username = cfg.setting_str("username");
        let password = cfg.setting_str("password");
        if api_key.is_none() && (username.is_none() || password.is_none()) {
            return Err(SourceError::Config(
                "jellyfin source requires either `api_key` or both `username` and `password`"
                    .to_owned(),
            ));
        }
        Ok(Self {
            name: cfg.name.clone(),
            url,
            username,
            password,
            api_key,
            user_id: cfg.setting_str("user_id"),
            max_bitrate: cfg
                .setting_str("max_bitrate")
                .and_then(|s| s.parse::<u32>().ok()),
            format: cfg.setting_str("format"),
            accept_invalid_certs: setting_bool(cfg, "accept_invalid_certs", false),
        })
    }
}

// ─── Source ──────────────────────────────────────────────────────────────────

enum Credentials {
    ApiKey(String),
    Password { username: String, password: String },
}

struct Session {
    token: String,
    user_id: String,
}

/// A music source backed by a Jellyfin server.
pub struct JellyfinSource {
    name: String,
    base: String,
    http: reqwest::Client,
    credentials: Credentials,
    configured_user_id: Option<String>,
    max_bitrate: Option<u32>,
    format: Option<String>,
    device_id: String,
    session: Mutex<Option<Arc<Session>>>,
}

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS` under `feature = "jellyfin"`.
pub fn jellyfin_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let jc = JellyfinConfig::from_source_config(cfg)?;
    let http = http_client(Duration::from_secs(30), jc.accept_invalid_certs)?;
    let credentials = if let Some(key) = jc.api_key {
        Credentials::ApiKey(key)
    } else {
        Credentials::Password {
            username: jc.username.ok_or_else(|| {
                SourceError::Config("jellyfin source missing `username`".to_owned())
            })?,
            password: jc.password.ok_or_else(|| {
                SourceError::Config("jellyfin source missing `password`".to_owned())
            })?,
        }
    };
    Ok(Box::new(JellyfinSource {
        device_id: format!("rmpd-{}", jc.name),
        name: jc.name,
        base: jc.url,
        http,
        credentials,
        configured_user_id: jc.user_id,
        max_bitrate: jc.max_bitrate,
        format: jc.format,
        session: Mutex::new(None),
    }))
}

impl JellyfinSource {
    /// Return the cached session, logging in if there is none. The lock is
    /// held across the login so concurrent callers share a single attempt.
    async fn session(&self) -> SourceResult<Arc<Session>> {
        let mut slot = self.session.lock().await;
        if let Some(s) = slot.as_ref() {
            return Ok(Arc::clone(s));
        }
        let s = Arc::new(self.login().await?);
        *slot = Some(Arc::clone(&s));
        Ok(s)
    }

    /// Drop `stale` from the cache unless another task already replaced it.
    async fn invalidate(&self, stale: &Arc<Session>) {
        let mut slot = self.session.lock().await;
        if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, stale)) {
            *slot = None;
        }
    }

    async fn login(&self) -> SourceResult<Session> {
        match &self.credentials {
            Credentials::ApiKey(key) => {
                let user_id = match &self.configured_user_id {
                    Some(u) => u.clone(),
                    None => self.first_user_id(key).await?,
                };
                Ok(Session {
                    token: key.clone(),
                    user_id,
                })
            }
            Credentials::Password { username, password } => {
                let body = serde_json::json!({ "Username": username, "Pw": password }).to_string();
                let header = auth_header(&self.device_id, None);
                let req = self
                    .http
                    .post(format!("{}/Users/AuthenticateByName", self.base))
                    .header("X-Emby-Authorization", header.clone())
                    .header("Authorization", header)
                    .header("Content-Type", "application/json")
                    .body(body);
                // A bad password is a 401 from this endpoint (-> `Auth`).
                let bytes = send_bytes(req).await?;
                let auth: AuthResult = serde_json::from_slice(&bytes).map_err(|e| {
                    SourceError::Protocol(format!("invalid Jellyfin auth response: {e}"))
                })?;
                if auth.access_token.is_empty() {
                    return Err(SourceError::Auth(
                        "Jellyfin returned no access token".to_owned(),
                    ));
                }
                // A user token can only act as the user it was issued for.
                let user_id = if auth.user.id.is_empty() {
                    self.configured_user_id.clone().unwrap_or_default()
                } else {
                    auth.user.id
                };
                if user_id.is_empty() {
                    return Err(SourceError::Protocol(
                        "Jellyfin returned no user id".to_owned(),
                    ));
                }
                Ok(Session {
                    token: auth.access_token,
                    user_id,
                })
            }
        }
    }

    /// With a bare API key there is no user context: take the first user.
    async fn first_user_id(&self, key: &str) -> SourceResult<String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "PascalCase", default)]
        struct U {
            id: String,
        }
        let header = auth_header(&self.device_id, Some(key));
        let req = self
            .http
            .get(format!("{}/Users", self.base))
            .header("X-Emby-Authorization", header.clone())
            .header("Authorization", header)
            .header("X-Emby-Token", key);
        let bytes = send_bytes(req).await?;
        let users: Vec<U> = serde_json::from_slice(&bytes)
            .map_err(|e| SourceError::Protocol(format!("invalid Jellyfin users response: {e}")))?;
        users
            .into_iter()
            .map(|u| u.id)
            .find(|i| !i.is_empty())
            .ok_or_else(|| {
                SourceError::Config(
                    "jellyfin: cannot determine a user; set `user_id` with `api_key`".to_owned(),
                )
            })
    }

    /// Authenticated GET of `path_and_query` (relative to the base URL).
    ///
    /// With password credentials a 401 means the token expired or was revoked:
    /// the cached session is discarded, one fresh login is made and the
    /// request retried once.
    async fn get(&self, path_and_query: &str) -> SourceResult<Vec<u8>> {
        let session = self.session().await?;
        match self.get_with(&session, path_and_query).await {
            Err(SourceError::Auth(_))
                if matches!(self.credentials, Credentials::Password { .. }) =>
            {
                self.invalidate(&session).await;
                let fresh = self.session().await?;
                self.get_with(&fresh, path_and_query).await
            }
            other => other,
        }
    }

    async fn get_with(&self, session: &Session, path_and_query: &str) -> SourceResult<Vec<u8>> {
        let header = auth_header(&self.device_id, Some(&session.token));
        let req = self
            .http
            .get(format!("{}{}", self.base, path_and_query))
            .header("X-Emby-Authorization", header.clone())
            .header("Authorization", header)
            .header("X-Emby-Token", session.token.as_str());
        send_bytes(req).await
    }

    async fn get_items(&self, path_and_query: &str) -> SourceResult<Vec<BaseItem>> {
        parse_items(&self.get(path_and_query).await?)
    }

    async fn user_id(&self) -> SourceResult<String> {
        Ok(self.session().await?.user_id.clone())
    }

    async fn fetch_item(&self, id: &str) -> SourceResult<Option<BaseItem>> {
        let uid = self.user_id().await?;
        let items = self
            .get_items(&format!(
                "/Items?userId={}&Ids={}&Fields={ITEM_FIELDS}",
                urlenc(&uid),
                urlenc(id)
            ))
            .await?;
        Ok(items.into_iter().next())
    }

    fn songs(&self, items: &[BaseItem]) -> Vec<Song> {
        items
            .iter()
            .filter(|i| i.item_type.as_deref().is_none_or(|t| t == "Audio"))
            .map(|i| map_item(&self.name, i))
            .collect()
    }
}

#[async_trait]
impl MusicSource for JellyfinSource {
    fn scheme(&self) -> &str {
        "jellyfin"
    }

    fn name(&self) -> &str {
        &self.name
    }

    /// Public server info probe, then an authentication check.
    async fn ping(&self) -> SourceResult<()> {
        send_bytes(self.http.get(format!("{}/System/Info/Public", self.base))).await?;
        self.session().await.map(|_| ())
    }

    /// Root lists album artists; deeper levels are served from the database
    /// after a catalog sync (like the Subsonic backend).
    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        if !dir_segments(dir).is_empty() {
            return Ok(Vec::new());
        }
        let uid = self.user_id().await?;
        let items = self
            .get_items(&format!(
                "/Artists/AlbumArtists?userId={}&Recursive=true&SortBy=SortName&SortOrder=Ascending",
                urlenc(&uid)
            ))
            .await?;
        Ok(items
            .iter()
            .filter_map(|a| a.name.as_deref())
            .map(|n| SourceEntry::Dir(format!("{}/{}", self.name, enc(n))))
            .collect())
    }

    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        let uid = self.user_id().await?;
        let mut songs = Vec::new();
        let mut start = 0usize;
        loop {
            let items = self
                .get_items(&format!(
                    "/Items?userId={}&IncludeItemTypes=Audio&Recursive=true&Fields={ITEM_FIELDS}\
                     &SortBy=AlbumArtist,Album,ParentIndexNumber,IndexNumber,SortName\
                     &SortOrder=Ascending&StartIndex={start}&Limit={PAGE}\
                     &EnableTotalRecordCount=false",
                    urlenc(&uid)
                ))
                .await?;
            if items.is_empty() {
                break;
            }
            start += items.len();
            songs.extend(self.songs(&items));
        }
        Ok(songs)
    }

    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let uid = self.user_id().await?;
        let items = self
            .get_items(&format!(
                "/Items?userId={}&IncludeItemTypes=Audio&Recursive=true&Fields={ITEM_FIELDS}\
                 &SearchTerm={}&Limit=100",
                urlenc(&uid),
                urlenc(query)
            ))
            .await?;
        Ok(self.songs(&items))
    }

    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        let session = self.session().await?;
        Ok(build_stream_url(
            &self.base,
            song_id,
            &session.token,
            &session.user_id,
            &self.device_id,
            self.max_bitrate,
            self.format.as_deref(),
        ))
    }

    /// Primary image of the track, falling back to its album's image. Missing
    /// art is `Ok(None)`.
    async fn cover_art(&self, song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        let primary = |id: &str| format!("/Items/{}/Images/Primary?maxWidth=600", urlenc(id));
        match self.get(&primary(song_id)).await {
            Ok(b) if !b.is_empty() => return Ok(Some(b)),
            Ok(_) | Err(SourceError::NotFound(_)) => {}
            Err(e) => {
                tracing::debug!(
                    "jellyfin source '{}': no cover art for {} ({e})",
                    self.name,
                    song_id
                );
                return Ok(None);
            }
        }
        let album_id = match self.fetch_item(song_id).await {
            Ok(Some(it)) => it.album_id,
            _ => None,
        };
        let Some(album_id) = album_id else {
            return Ok(None);
        };
        match self.get(&primary(&album_id)).await {
            Ok(b) if !b.is_empty() => Ok(Some(b)),
            _ => Ok(None),
        }
    }

    async fn lookup(&self, uri: &str) -> SourceResult<Option<Song>> {
        let id = leaf_id(uri);
        if id.is_empty() {
            return Ok(None);
        }
        Ok(self
            .fetch_item(id)
            .await?
            .map(|it| map_item(&self.name, &it)))
    }

    async fn playlists(&self) -> SourceResult<Vec<String>> {
        let uid = self.user_id().await?;
        let items = self
            .get_items(&format!(
                "/Items?userId={}&IncludeItemTypes=Playlist&Recursive=true\
                 &SortBy=SortName&SortOrder=Ascending",
                urlenc(&uid)
            ))
            .await?;
        Ok(items.into_iter().filter_map(|i| i.name).collect())
    }

    async fn playlist_items(&self, name: &str) -> SourceResult<Vec<Song>> {
        let uid = self.user_id().await?;
        let lists = self
            .get_items(&format!(
                "/Items?userId={}&IncludeItemTypes=Playlist&Recursive=true",
                urlenc(&uid)
            ))
            .await?;
        let id = lists
            .iter()
            .find(|p| p.name.as_deref() == Some(name))
            .map(|p| p.id.clone())
            .ok_or_else(|| SourceError::NotFound(format!("playlist: {name}")))?;
        let items = self
            .get_items(&format!(
                "/Playlists/{}/Items?userId={}&Fields={ITEM_FIELDS}",
                urlenc(&id),
                urlenc(&uid)
            ))
            .await?;
        Ok(self.songs(&items))
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ITEMS_FIXTURE: &str = r#"{
      "Items": [
        {
          "Name": "Roads", "ServerId": "s", "Id": "a1b2c3d4e5f60718293a4b5c6d7e8f90",
          "RunTimeTicks": 2400000000, "ProductionYear": 1994,
          "IndexNumber": 3, "ParentIndexNumber": 1,
          "Container": "flac", "Artists": ["Portishead"], "AlbumArtist": "Portishead",
          "Album": "Dummy", "AlbumId": "alb1", "Genres": ["Trip-Hop", "Electronic"],
          "MediaSources": [{"Container": "flac", "Bitrate": 987000}],
          "Type": "Audio", "UnknownField": {"x": 1}
        },
        {
          "Name": "AC/DC Medley", "Id": "ff00", "RunTimeTicks": null,
          "Artists": null, "Genres": null, "AlbumArtist": "AC/DC", "Album": "Live 100%",
          "MediaSources": [{"Container": "mov,mp4,m4a,3gp,3g2,mj2", "Bitrate": 256000}],
          "Type": "Audio"
        }
      ],
      "TotalRecordCount": 2, "StartIndex": 0
    }"#;

    #[test]
    fn parses_items_page() {
        let items = parse_items(ITEMS_FIXTURE.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name.as_deref(), Some("Roads"));
        assert_eq!(items[0].run_time_ticks, Some(2_400_000_000));
        assert!(items[1].artists.is_empty());
        assert!(parse_items(b"not json").is_err());
        assert!(parse_items(br#"{"Items":null}"#).unwrap().is_empty());
    }

    #[test]
    fn maps_full_item() {
        let items = parse_items(ITEMS_FIXTURE.as_bytes()).unwrap();
        let s = map_item("jf", &items[0]);
        assert_eq!(
            s.path.as_str(),
            "jf/Portishead/Dummy/a1b2c3d4e5f60718293a4b5c6d7e8f90.flac"
        );
        assert_eq!(s.duration, Some(Duration::from_secs(240)));
        assert_eq!(s.bitrate, Some(987));
        let tag = |k: &str| -> Vec<&str> {
            s.tags
                .iter()
                .filter(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .collect()
        };
        assert_eq!(tag("title"), vec!["Roads"]);
        assert_eq!(tag("album"), vec!["Dummy"]);
        assert_eq!(tag("track"), vec!["3"]);
        assert_eq!(tag("disc"), vec!["1"]);
        assert_eq!(tag("date"), vec!["1994"]);
        assert_eq!(tag("genre"), vec!["Trip-Hop", "Electronic"]);
    }

    #[test]
    fn maps_sparse_item_with_encoded_path() {
        let items = parse_items(ITEMS_FIXTURE.as_bytes()).unwrap();
        let s = map_item("jf", &items[1]);
        assert_eq!(s.path.as_str(), "jf/AC%2FDC/Live 100%25/ff00.mp4");
        assert_eq!(s.duration, None);
        assert_eq!(leaf_id(s.path.as_str()), "ff00");
    }

    #[test]
    fn stream_url_static_and_transcoded() {
        let u = build_stream_url("https://jf", "abc", "tok en", "u1", "dev", None, None);
        assert_eq!(
            u,
            "https://jf/Audio/abc/stream?static=true&api_key=tok%20en"
        );
        let t = build_stream_url(
            "https://jf",
            "abc",
            "tok",
            "u1",
            "dev",
            Some(192),
            Some("MP3"),
        );
        assert!(t.starts_with("https://jf/Audio/abc/universal?UserId=u1&DeviceId=dev&api_key=tok"));
        assert!(t.contains("&Container=mp3&TranscodingContainer=mp3"));
        assert!(t.contains("&AudioCodec=mp3"));
        assert!(t.ends_with("&MaxStreamingBitrate=192000"));
        let b = build_stream_url("https://jf", "abc", "tok", "u1", "dev", Some(128), None);
        assert!(b.contains(&format!("&Container={DIRECT_CONTAINERS}")));
    }

    #[test]
    fn auth_header_format() {
        let h = auth_header("rmpd-home", None);
        assert!(h.starts_with("MediaBrowser Client=\"rmpd\", Device=\"rmpd\""));
        assert!(h.contains("DeviceId=\"rmpd-home\""));
        assert!(!h.contains("Token"));
        let t = auth_header("d", Some("secret"));
        assert!(t.ends_with(", Token=\"secret\""));
    }

    #[test]
    fn parses_auth_result() {
        let a: AuthResult = serde_json::from_str(
            r#"{"User":{"Name":"me","Id":"user-1"},"AccessToken":"tok","ServerId":"s"}"#,
        )
        .unwrap();
        assert_eq!(a.access_token, "tok");
        assert_eq!(a.user.id, "user-1");
    }

    fn cfg(entries: &[(&str, &str)]) -> SourceConfig {
        let mut t = toml::Table::new();
        for (k, v) in entries {
            t.insert((*k).to_owned(), toml::Value::String((*v).to_owned()));
        }
        SourceConfig {
            name: "jf".to_owned(),
            source_type: "jellyfin".to_owned(),
            enabled: true,
            settings: t,
        }
    }

    #[test]
    fn config_validation() {
        assert!(JellyfinConfig::from_source_config(&cfg(&[])).is_err());
        assert!(JellyfinConfig::from_source_config(&cfg(&[("url", "https://jf")])).is_err());
        assert!(
            JellyfinConfig::from_source_config(&cfg(&[("url", "https://jf"), ("username", "u"),]))
                .is_err()
        );
        let ok = JellyfinConfig::from_source_config(&cfg(&[
            ("url", "https://jf/"),
            ("username", "u"),
            ("password", "hunter2"),
            ("max_bitrate", "256"),
        ]))
        .unwrap();
        assert_eq!(ok.url, "https://jf");
        assert_eq!(ok.max_bitrate, Some(256));
        let dbg = format!("{ok:?}");
        assert!(!dbg.contains("hunter2"));
        assert!(
            JellyfinConfig::from_source_config(&cfg(&[("url", "https://jf"), ("api_key", "k")]))
                .is_ok()
        );
    }

    #[test]
    fn factory_builds_without_io() {
        let src = jellyfin_source_factory(&cfg(&[("url", "https://jf"), ("api_key", "k")]));
        assert!(src.is_ok());
        let src = src.ok().unwrap();
        assert_eq!(src.scheme(), "jellyfin");
        assert_eq!(src.name(), "jf");
    }

    /// Minimal HTTP server: logins hand out `t1`, `t2`, ...; only `t2` is
    /// accepted on GET. Returns the request lines it saw.
    fn spawn_server(connections: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut logins = 0;
            for _ in 0..connections {
                let (mut conn, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                let head_end = loop {
                    let n = conn.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                    assert!(n > 0, "connection closed early");
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = conn.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                let line = head.lines().next().unwrap_or_default().to_owned();
                let (status, body) = if line.starts_with("post ") {
                    logins += 1;
                    (
                        "200 OK",
                        format!(r#"{{"AccessToken":"t{logins}","User":{{"Id":"u1"}}}}"#),
                    )
                } else if head.contains("x-emby-token: t2") {
                    ("200 OK", r#"{"Items":[]}"#.to_owned())
                } else {
                    ("401 Unauthorized", String::new())
                };
                seen.push(format!("{line} -> {status}"));
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                conn.write_all(resp.as_bytes()).unwrap();
            }
            seen
        });
        (base, handle)
    }

    fn password_source(base: String) -> JellyfinSource {
        JellyfinSource {
            name: "jf".to_owned(),
            base,
            http: http_client(Duration::from_secs(5), false).unwrap(),
            credentials: Credentials::Password {
                username: "u".to_owned(),
                password: "p".to_owned(),
            },
            configured_user_id: None,
            max_bitrate: None,
            format: None,
            device_id: "rmpd-jf".to_owned(),
            session: Mutex::new(None),
        }
    }

    #[tokio::test]
    async fn relogs_in_once_on_401_with_password() {
        // login, GET (401), re-login, GET (200)
        let (base, server) = spawn_server(4);
        let src = password_source(base);
        let body = src.get("/Items").await.unwrap();
        assert_eq!(body, br#"{"Items":[]}"#);
        // The refreshed session is cached: no further login.
        assert_eq!(src.session().await.unwrap().token, "t2");
        let seen = server.join().unwrap();
        assert_eq!(seen.iter().filter(|l| l.starts_with("post ")).count(), 2);
    }

    #[tokio::test]
    async fn persistent_401_is_retried_only_once() {
        // stale GET (401), login (t1), GET (401) -> error, no third attempt.
        let (base, server) = spawn_server(3);
        let src = password_source(base);
        *src.session.lock().await = Some(Arc::new(Session {
            token: "stale".to_owned(),
            user_id: "u1".to_owned(),
        }));
        let err = src.get("/Items").await.unwrap_err();
        assert!(matches!(err, SourceError::Auth(_)));
        assert_eq!(server.join().unwrap().len(), 3);
    }
}
