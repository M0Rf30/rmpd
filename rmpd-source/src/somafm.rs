// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `SomaFmSource` — listener-supported [SomaFM](https://somafm.com/) channels.
//!
//! Compiled with `feature = "radio"` (registered as source type `somafm`).
//! [`SyncPolicy::OnDemand`]: the mount root lists every channel as a live
//! stream; the channel list (`channels.json`) is cached for ten minutes.
//! Playback resolves the channel's `.pls` playlist to a direct stream URL.

use crate::common::{http_client, known_ext, leaf_id, make_song, send_bytes};
use async_trait::async_trait;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult, SyncPolicy};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Setting keys accepted in a `[[source]] type = "somafm"` block.
pub const SETTINGS: &[&str] = &["api_url", "format"];

const DEFAULT_API_URL: &str = "https://somafm.com/channels.json";
const CACHE_TTL: Duration = Duration::from_secs(600);

// ─── Wire types ──────────────────────────────────────────────────────────────

fn vec_or_null<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChannelsResponse {
    #[serde(deserialize_with = "vec_or_null")]
    channels: Vec<Channel>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct Channel {
    id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    dj: Option<String>,
    /// `|`-separated genres, e.g. `"ambient|electronic"`.
    genre: Option<String>,
    image: Option<String>,
    largeimage: Option<String>,
    xlimage: Option<String>,
    #[serde(deserialize_with = "vec_or_null")]
    playlists: Vec<PlaylistRef>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct PlaylistRef {
    url: Option<String>,
    /// `mp3`, `aac` or `aacp`.
    format: Option<String>,
    /// `highest`, `high` or `low`.
    quality: Option<String>,
}

fn parse_channels(body: &[u8]) -> SourceResult<Vec<Channel>> {
    serde_json::from_slice::<ChannelsResponse>(body)
        .map(|r| {
            r.channels
                .into_iter()
                .filter(|c| c.id.as_deref().is_some_and(|i| !i.trim().is_empty()))
                .collect()
        })
        .map_err(|e| SourceError::Protocol(format!("invalid SomaFM response: {e}")))
}

// ─── Logic ───────────────────────────────────────────────────────────────────

fn quality_rank(q: Option<&str>) -> u8 {
    match q.map(str::to_ascii_lowercase).as_deref() {
        Some("highest") => 3,
        Some("high") => 2,
        Some("low") => 1,
        _ => 0,
    }
}

/// Pick the best playlist: the preferred `format` first (highest quality),
/// otherwise the best of any format.
fn choose_playlist<'a>(ch: &'a Channel, format: &str) -> Option<&'a PlaylistRef> {
    let usable = |p: &&PlaylistRef| p.url.as_deref().is_some_and(|u| !u.trim().is_empty());
    let best = |it: Vec<&'a PlaylistRef>| {
        it.into_iter()
            .max_by_key(|p| quality_rank(p.quality.as_deref()))
    };
    let same: Vec<&PlaylistRef> = ch
        .playlists
        .iter()
        .filter(usable)
        .filter(|p| {
            p.format
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case(format))
        })
        .collect();
    best(same).or_else(|| best(ch.playlists.iter().filter(usable).collect()))
}

/// Case-insensitive match on title, description, DJ and genre.
fn matches_query(ch: &Channel, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    [&ch.title, &ch.description, &ch.dj, &ch.genre]
        .into_iter()
        .flatten()
        .any(|f| f.to_lowercase().contains(&q))
}

fn map_channel(source: &str, format: &str, ch: &Channel) -> Option<Song> {
    let id = ch.id.as_deref()?.trim();
    if id.is_empty() {
        return None;
    }
    let ext = known_ext(match format.to_ascii_lowercase().as_str() {
        "aacp" => "aac",
        _ => format,
    });
    let leaf = match ext {
        Some(e) => format!("{id}.{e}"),
        None => id.to_owned(),
    };
    let title = ch.title.as_deref().unwrap_or(id).trim().to_owned();
    let mut tags: Vec<(&str, String)> = vec![
        ("title", title.clone()),
        ("name", title),
        ("comment", ch.description.clone().unwrap_or_default()),
    ];
    for g in ch
        .genre
        .as_deref()
        .unwrap_or("")
        .split('|')
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        tags.push(("genre", g.to_owned()));
    }
    Some(make_song(format!("{source}/{leaf}"), tags, None, None))
}

fn cover_url(ch: &Channel) -> Option<&str> {
    [&ch.largeimage, &ch.xlimage, &ch.image]
        .into_iter()
        .flatten()
        .map(|s| s.trim())
        .find(|u| u.starts_with("http://") || u.starts_with("https://"))
}

// ─── Source ──────────────────────────────────────────────────────────────────

/// Source exposing SomaFM channels as live radio streams.
pub struct SomaFmSource {
    name: String,
    api_url: String,
    format: String,
    http: reqwest::Client,
    cache: Mutex<Option<(Instant, Arc<Vec<Channel>>)>>,
}

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS` under `feature = "radio"`.
pub fn somafm_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let api_url = match cfg.setting_str("api_url") {
        Some(u) => {
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return Err(SourceError::Config(
                    "somafm `api_url` must start with http:// or https://".to_owned(),
                ));
            }
            u
        }
        None => DEFAULT_API_URL.to_owned(),
    };
    let format = cfg
        .setting_str("format")
        .map(|f| f.to_ascii_lowercase())
        .unwrap_or_else(|| "mp3".to_owned());
    if !matches!(format.as_str(), "mp3" | "aac" | "aacp") {
        return Err(SourceError::Config(
            "somafm `format` must be one of mp3, aac, aacp".to_owned(),
        ));
    }
    Ok(Box::new(SomaFmSource {
        name: cfg.name.clone(),
        api_url,
        format,
        http: http_client(Duration::from_secs(20), false)?,
        cache: Mutex::new(None),
    }))
}

impl SomaFmSource {
    /// Channel list, cached for [`CACHE_TTL`].
    async fn channels(&self) -> SourceResult<Arc<Vec<Channel>>> {
        let mut guard = self.cache.lock().await;
        if let Some((at, list)) = guard.as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(Arc::clone(list));
        }
        let body = send_bytes(self.http.get(&self.api_url)).await?;
        let list = Arc::new(parse_channels(&body)?);
        *guard = Some((Instant::now(), Arc::clone(&list)));
        Ok(list)
    }

    fn songs(&self, channels: &[Channel], query: &str) -> Vec<Song> {
        channels
            .iter()
            .filter(|c| matches_query(c, query))
            .filter_map(|c| map_channel(&self.name, &self.format, c))
            .collect()
    }

    async fn channel(&self, id: &str) -> SourceResult<Channel> {
        self.channels()
            .await?
            .iter()
            .find(|c| c.id.as_deref().map(str::trim) == Some(id))
            .cloned()
            .ok_or_else(|| SourceError::NotFound(format!("somafm channel: {id}")))
    }
}

#[async_trait]
impl MusicSource for SomaFmSource {
    fn scheme(&self) -> &str {
        "somafm"
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn ping(&self) -> SourceResult<()> {
        self.channels().await.map(|_| ())
    }

    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        if !dir.trim_matches('/').is_empty() {
            return Err(SourceError::NotFound(format!("somafm directory: {dir}")));
        }
        let channels = self.channels().await?;
        Ok(self
            .songs(&channels, "")
            .into_iter()
            .map(SourceEntry::Song)
            .collect())
    }

    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        let channels = self.channels().await?;
        Ok(self.songs(&channels, ""))
    }

    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let channels = self.channels().await?;
        Ok(self.songs(&channels, query))
    }

    /// Fetch the channel's `.pls` and return its first stream URL.
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        let ch = self.channel(song_id).await?;
        let pl = choose_playlist(&ch, &self.format)
            .and_then(|p| p.url.clone())
            .ok_or_else(|| SourceError::NotFound(format!("no playlist for channel: {song_id}")))?;
        let body = send_bytes(self.http.get(pl.trim())).await?;
        let text = String::from_utf8_lossy(&body);
        first_stream_uri(&text)
            .ok_or_else(|| SourceError::Protocol("empty SomaFM playlist".to_owned()))
    }

    async fn cover_art(&self, song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        let Ok(ch) = self.channel(song_id).await else {
            return Ok(None);
        };
        let Some(url) = cover_url(&ch) else {
            return Ok(None);
        };
        match send_bytes(self.http.get(url)).await {
            Ok(b) if !b.is_empty() => Ok(Some(b)),
            _ => Ok(None),
        }
    }

    async fn lookup(&self, uri: &str) -> SourceResult<Option<Song>> {
        let id = leaf_id(uri);
        match self.channel(id).await {
            Ok(ch) => Ok(map_channel(&self.name, &self.format, &ch)),
            Err(SourceError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn is_live(&self, _song_id: &str) -> bool {
        true
    }

    fn sync_policy(&self) -> SyncPolicy {
        SyncPolicy::OnDemand
    }
}

/// First stream URL of a PLS (or bare-URL) playlist body.
fn first_stream_uri(text: &str) -> Option<String> {
    if let Some(parser) = rmpd_plugin::playlist::parser_by_name("pls")
        && let Some(e) = parser.parse("", text).into_iter().next()
        && !e.uri.trim().is_empty()
    {
        return Some(e.uri.trim().to_owned());
    }
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("http://") || l.starts_with("https://"))
        .map(str::to_owned)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNELS: &str = r#"{"channels":[
      {"id":"groovesalad","title":"Groove Salad","description":"A nicely chilled plate of ambient/downtempo beats",
       "dj":"Rusty Hodge","genre":"ambient|electronic","image":"https://somafm.com/img/gs120.jpg",
       "largeimage":"https://somafm.com/logos/256/gs256.jpg","xlimage":"https://somafm.com/logos/512/gs512.jpg",
       "listeners":"1234",
       "playlists":[
         {"url":"https://somafm.com/groovesalad130.pls","format":"aac","quality":"low"},
         {"url":"https://somafm.com/groovesalad256.pls","format":"mp3","quality":"highest"},
         {"url":"https://somafm.com/groovesalad64.pls","format":"aacp","quality":"high"},
         {"url":"https://somafm.com/groovesalad32.pls","format":"mp3","quality":"low"}]},
      {"id":"defcon","title":"DEF CON Radio","description":"Music for Hacking","dj":"Dj Dan","genre":"electronic",
       "playlists":[{"url":"https://somafm.com/defcon.pls","format":"aacp","quality":"high"}]},
      {"id":"","title":"Broken"},
      {"title":"No id"}
    ]}"#;

    #[test]
    fn parses_channels_and_drops_invalid() {
        let ch = parse_channels(CHANNELS.as_bytes()).unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].id.as_deref(), Some("groovesalad"));
        assert_eq!(ch[0].playlists.len(), 4);
        assert!(parse_channels(b"nope").is_err());
        assert!(parse_channels(br#"{"channels":null}"#).unwrap().is_empty());
    }

    #[test]
    fn chooses_best_playlist_for_format() {
        let ch = parse_channels(CHANNELS.as_bytes()).unwrap();
        let mp3 = choose_playlist(&ch[0], "mp3").unwrap();
        assert_eq!(
            mp3.url.as_deref(),
            Some("https://somafm.com/groovesalad256.pls")
        );
        let aacp = choose_playlist(&ch[0], "AACP").unwrap();
        assert_eq!(
            aacp.url.as_deref(),
            Some("https://somafm.com/groovesalad64.pls")
        );
        // Only aacp available for defcon: falls back to it for an mp3 request.
        let fb = choose_playlist(&ch[1], "mp3").unwrap();
        assert_eq!(fb.url.as_deref(), Some("https://somafm.com/defcon.pls"));
        assert!(choose_playlist(&Channel::default(), "mp3").is_none());
    }

    #[test]
    fn maps_channel_to_live_song() {
        let ch = parse_channels(CHANNELS.as_bytes()).unwrap();
        let s = map_channel("soma", "mp3", &ch[0]).unwrap();
        assert_eq!(s.path.as_str(), "soma/groovesalad.mp3");
        assert_eq!(s.duration, None);
        assert!(
            s.tags
                .iter()
                .any(|(k, v)| k == "title" && v == "Groove Salad")
        );
        let genres: Vec<&str> = s
            .tags
            .iter()
            .filter(|(k, _)| k == "genre")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(genres, vec!["ambient", "electronic"]);
        let aacp = map_channel("soma", "aacp", &ch[1]).unwrap();
        assert_eq!(aacp.path.as_str(), "soma/defcon.aac");
        assert_eq!(leaf_id(aacp.path.as_str()), "defcon");
    }

    #[test]
    fn query_matching_and_cover() {
        let ch = parse_channels(CHANNELS.as_bytes()).unwrap();
        assert!(matches_query(&ch[0], "SALAD"));
        assert!(matches_query(&ch[0], "rusty"));
        assert!(matches_query(&ch[1], "hacking"));
        assert!(!matches_query(&ch[1], "salad"));
        assert!(matches_query(&ch[1], "  "));
        assert_eq!(
            cover_url(&ch[0]),
            Some("https://somafm.com/logos/256/gs256.jpg")
        );
        assert_eq!(cover_url(&ch[1]), None);
    }

    #[test]
    fn first_stream_uri_from_pls() {
        let pls = "[playlist]\nNumberOfEntries=2\nFile1=http://ice1.somafm.com/groovesalad-256-mp3\nTitle1=SomaFM\nLength1=-1\nFile2=http://ice2.somafm.com/groovesalad-256-mp3\nTitle2=SomaFM\nLength2=-1\nVersion=2\n";
        assert_eq!(
            first_stream_uri(pls).as_deref(),
            Some("http://ice1.somafm.com/groovesalad-256-mp3")
        );
        assert_eq!(
            first_stream_uri("# comment\nhttps://s.example/live\n").as_deref(),
            Some("https://s.example/live")
        );
        assert_eq!(first_stream_uri(""), None);
    }

    fn cfg(entries: &[(&str, &str)]) -> SourceConfig {
        let mut t = toml::Table::new();
        for (k, v) in entries {
            t.insert((*k).to_owned(), toml::Value::String((*v).to_owned()));
        }
        SourceConfig {
            name: "soma".to_owned(),
            source_type: "somafm".to_owned(),
            enabled: true,
            settings: t,
        }
    }

    #[test]
    fn factory_validates_settings() {
        let src = somafm_source_factory(&cfg(&[])).ok().unwrap();
        assert_eq!(src.scheme(), "somafm");
        assert_eq!(src.sync_policy(), SyncPolicy::OnDemand);
        assert!(src.is_live("x"));
        assert!(somafm_source_factory(&cfg(&[("format", "wav")])).is_err());
        assert!(somafm_source_factory(&cfg(&[("api_url", "ftp://x")])).is_err());
        assert!(somafm_source_factory(&cfg(&[("format", "AAC")])).is_ok());
    }
}
