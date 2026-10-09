// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `RadioSource` — internet radio via the community
//! [Radio-Browser](https://www.radio-browser.info/) API.
//!
//! Compiled only when `feature = "radio"` is active. [`SyncPolicy::OnDemand`]:
//! nothing is mirrored into the database; `lsinfo` under the mount browses:
//!
//! ```text
//! <name>/favourites            stations listed in the `stations` setting
//! <name>/top-voted             most voted stations
//! <name>/top-clicked           most clicked stations
//! <name>/countries/<country>   stations by country
//! <name>/tags/<tag>            stations by tag
//! ```
//!
//! The API server is discovered through `all.api.radio-browser.info` unless
//! `api_url` pins one. Every station is a live stream ([`MusicSource::is_live`]).

use crate::common::{
    dir_segments, enc, http_client, known_ext, leaf_id, make_song, normalize_base_url, send_bytes,
    setting_list, urlenc,
};
use async_trait::async_trait;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult, SyncPolicy};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::OnceCell;

/// Setting keys accepted in a `[[source]] type = "radio"` block.
pub const SETTINGS: &[&str] = &["api_url", "stations", "limit", "hide_broken"];

/// Bootstrap host used for API server discovery (and as last-resort server).
const DISCOVERY_HOST: &str = "https://all.api.radio-browser.info";

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1000;

// ─── Wire types ──────────────────────────────────────────────────────────────

/// One Radio-Browser station. Every field is optional: servers return `null`
/// for unknown values.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct Station {
    stationuuid: Option<String>,
    name: Option<String>,
    url: Option<String>,
    url_resolved: Option<String>,
    homepage: Option<String>,
    favicon: Option<String>,
    tags: Option<String>,
    country: Option<String>,
    codec: Option<String>,
    bitrate: Option<u32>,
}

impl Station {
    fn uuid(&self) -> &str {
        self.stationuuid.as_deref().unwrap_or("")
    }

    /// Resolved stream URL when known, else the registered one.
    fn stream_url(&self) -> Option<&str> {
        [self.url_resolved.as_deref(), self.url.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|u| !u.is_empty())
    }
}

/// Entry of `/json/countries` and `/json/tags`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Named {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ServerEntry {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClickResponse {
    url: Option<String>,
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> SourceResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SourceError::Protocol(format!("invalid Radio-Browser response: {e}")))
}

fn parse_stations(body: &[u8]) -> SourceResult<Vec<Station>> {
    parse_json(body)
}

/// Names from `/json/countries` or `/json/tags` (empty names dropped).
fn parse_names(body: &[u8]) -> SourceResult<Vec<String>> {
    Ok(parse_json::<Vec<Named>>(body)?
        .into_iter()
        .filter_map(|n| n.name)
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect())
}

/// Server base URLs (`https://<host>`) from `/json/servers`, deduplicated.
fn parse_servers(body: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(list) = serde_json::from_slice::<Vec<ServerEntry>>(body) {
        for host in list.into_iter().filter_map(|s| s.name) {
            let host = host.trim();
            if host.is_empty() {
                continue;
            }
            let url = format!("https://{host}");
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

/// Stream URL from a `/json/url/{uuid}` response.
fn parse_click_url(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<ClickResponse>(body)
        .ok()
        .and_then(|c| c.url)
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
}

// ─── Mapping ─────────────────────────────────────────────────────────────────

/// Convert a station to a live `Song`. `dir` is the (already percent-encoded)
/// browse directory below the mount; the path is `<source>/<dir>/<uuid>[.ext]`.
/// Returns `None` for entries without a uuid or stream URL.
fn map_station(source: &str, dir: &str, st: &Station) -> Option<Song> {
    let uuid = st.uuid().trim();
    if uuid.is_empty() || st.stream_url().is_none() {
        return None;
    }
    let ext = st
        .codec
        .as_deref()
        .map(|c| c.trim().trim_end_matches('+'))
        .and_then(known_ext);
    let leaf = match ext {
        Some(e) => format!("{uuid}.{e}"),
        None => uuid.to_owned(),
    };
    let path = if dir.is_empty() {
        format!("{source}/{leaf}")
    } else {
        format!("{source}/{dir}/{leaf}")
    };
    let name = st.name.as_deref().unwrap_or("").trim().to_owned();
    let mut tags: Vec<(&str, String)> = vec![("title", name.clone()), ("name", name)];
    for g in st
        .tags
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .take(3)
    {
        tags.push(("genre", g.to_owned()));
    }
    let bitrate = st.bitrate.filter(|b| *b > 0);
    // Live: no duration.
    Some(make_song(path, tags, None, bitrate))
}

fn map_stations(source: &str, dir: &str, stations: &[Station]) -> Vec<Song> {
    stations
        .iter()
        .filter_map(|s| map_station(source, dir, s))
        .collect()
}

/// Reorder `stations` to follow the configured `uuids` order (unknown last).
fn order_by_uuids(mut stations: Vec<Station>, uuids: &[String]) -> Vec<Station> {
    stations.sort_by_key(|s| {
        uuids
            .iter()
            .position(|u| u.eq_ignore_ascii_case(s.uuid()))
            .unwrap_or(usize::MAX)
    });
    stations
}

// ─── API paths ───────────────────────────────────────────────────────────────

fn list_params(limit: u32, hide_broken: bool) -> String {
    format!("hidebroken={hide_broken}&order=votes&reverse=true&limit={limit}")
}

fn path_by_country(country: &str, limit: u32, hide_broken: bool) -> String {
    format!(
        "/json/stations/bycountryexact/{}?{}",
        urlenc(country),
        list_params(limit, hide_broken)
    )
}

fn path_by_tag(tag: &str, limit: u32, hide_broken: bool) -> String {
    format!(
        "/json/stations/bytagexact/{}?{}",
        urlenc(tag),
        list_params(limit, hide_broken)
    )
}

fn path_search(query: &str, limit: u32, hide_broken: bool) -> String {
    format!(
        "/json/stations/search?name={}&{}",
        urlenc(query),
        list_params(limit, hide_broken)
    )
}

fn path_by_uuids(uuids: &[String]) -> String {
    let list = uuids
        .iter()
        .map(|u| urlenc(u))
        .collect::<Vec<_>>()
        .join(",");
    format!("/json/stations/byuuid?uuids={list}")
}

// ─── Source ──────────────────────────────────────────────────────────────────

/// Internet-radio source backed by the Radio-Browser API.
pub struct RadioSource {
    name: String,
    http: reqwest::Client,
    api_url: Option<String>,
    servers: OnceCell<Vec<String>>,
    favourites: Vec<String>,
    limit: u32,
    hide_broken: bool,
}

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS` under `feature = "radio"`.
pub fn radio_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let api_url = cfg
        .setting_str("api_url")
        .map(|u| normalize_base_url(&u, "radio"))
        .transpose()?;
    let limit = cfg
        .setting_str("limit")
        .and_then(|s| s.parse::<u32>().ok())
        .map_or(DEFAULT_LIMIT, |l| l.clamp(1, MAX_LIMIT));
    let hide_broken = crate::common::setting_bool(cfg, "hide_broken", true);
    Ok(Box::new(RadioSource {
        name: cfg.name.clone(),
        http: http_client(Duration::from_secs(20), false)?,
        api_url,
        servers: OnceCell::new(),
        favourites: setting_list(cfg, "stations"),
        limit,
        hide_broken,
    }))
}

impl RadioSource {
    /// API server base URLs, discovered once.
    async fn servers(&self) -> &Vec<String> {
        self.servers
            .get_or_init(|| async {
                if let Some(u) = &self.api_url {
                    return vec![u.clone()];
                }
                let discovered =
                    match send_bytes(self.http.get(format!("{DISCOVERY_HOST}/json/servers"))).await
                    {
                        Ok(body) => parse_servers(&body),
                        Err(e) => {
                            tracing::debug!(
                                "radio source '{}': server discovery failed ({e})",
                                self.name
                            );
                            Vec::new()
                        }
                    };
                if discovered.is_empty() {
                    return vec![DISCOVERY_HOST.to_owned()];
                }
                // Spread load: start at a pseudo-random server.
                let n = discovered.len();
                let start = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos() as usize)
                    % n;
                let mut rotated = discovered;
                rotated.rotate_left(start);
                rotated
            })
            .await
    }

    /// GET `path` on the first reachable API server.
    async fn api_get(&self, path: &str) -> SourceResult<Vec<u8>> {
        let mut last: Option<SourceError> = None;
        for base in self.servers().await {
            match send_bytes(self.http.get(format!("{base}{path}"))).await {
                Ok(b) => return Ok(b),
                Err(e @ SourceError::Unreachable(_)) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| SourceError::Unreachable("no Radio-Browser server".to_owned())))
    }

    async fn stations_at(&self, path: &str) -> SourceResult<Vec<Station>> {
        parse_stations(&self.api_get(path).await?)
    }

    async fn station_by_uuid(&self, uuid: &str) -> SourceResult<Option<Station>> {
        Ok(self
            .stations_at(&path_by_uuids(&[uuid.to_owned()]))
            .await?
            .into_iter()
            .next())
    }

    fn dirs(&self, segs: &[&str]) -> Vec<SourceEntry> {
        segs.iter()
            .map(|s| SourceEntry::Dir(format!("{}/{}", self.name, s)))
            .collect()
    }

    fn songs_entries(songs: Vec<Song>) -> Vec<SourceEntry> {
        songs.into_iter().map(SourceEntry::Song).collect()
    }
}

#[async_trait]
impl MusicSource for RadioSource {
    fn scheme(&self) -> &str {
        "radio"
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn ping(&self) -> SourceResult<()> {
        self.api_get("/json/stats").await.map(|_| ())
    }

    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        let segs = dir_segments(dir);
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let (limit, hb) = (self.limit, self.hide_broken);
        match segs.as_slice() {
            [] => {
                let mut root = Vec::new();
                if !self.favourites.is_empty() {
                    root.push("favourites");
                }
                root.extend(["top-voted", "top-clicked", "countries", "tags"]);
                Ok(self.dirs(&root))
            }
            ["favourites"] => {
                if self.favourites.is_empty() {
                    return Ok(Vec::new());
                }
                let found = self.stations_at(&path_by_uuids(&self.favourites)).await?;
                let ordered = order_by_uuids(found, &self.favourites);
                Ok(Self::songs_entries(map_stations(
                    &self.name,
                    "favourites",
                    &ordered,
                )))
            }
            ["top-voted"] => {
                let st = self
                    .stations_at(&format!("/json/stations/topvote/{limit}?hidebroken={hb}"))
                    .await?;
                Ok(Self::songs_entries(map_stations(
                    &self.name,
                    "top-voted",
                    &st,
                )))
            }
            ["top-clicked"] => {
                let st = self
                    .stations_at(&format!("/json/stations/topclick/{limit}?hidebroken={hb}"))
                    .await?;
                Ok(Self::songs_entries(map_stations(
                    &self.name,
                    "top-clicked",
                    &st,
                )))
            }
            ["countries"] => {
                let names = parse_names(
                    &self
                        .api_get(&format!("/json/countries?order=name&hidebroken={hb}"))
                        .await?,
                )?;
                Ok(names
                    .iter()
                    .map(|n| SourceEntry::Dir(format!("{}/countries/{}", self.name, enc(n))))
                    .collect())
            }
            ["tags"] => {
                let names = parse_names(
                    &self
                        .api_get(&format!(
                            "/json/tags?order=stationcount&reverse=true&hidebroken={hb}&limit=250"
                        ))
                        .await?,
                )?;
                Ok(names
                    .iter()
                    .map(|n| SourceEntry::Dir(format!("{}/tags/{}", self.name, enc(n))))
                    .collect())
            }
            ["countries", country] => {
                let st = self
                    .stations_at(&path_by_country(country, limit, hb))
                    .await?;
                let d = format!("countries/{}", enc(country));
                Ok(Self::songs_entries(map_stations(&self.name, &d, &st)))
            }
            ["tags", tag] => {
                let st = self.stations_at(&path_by_tag(tag, limit, hb)).await?;
                let d = format!("tags/{}", enc(tag));
                Ok(Self::songs_entries(map_stations(&self.name, &d, &st)))
            }
            _ => Err(SourceError::NotFound(format!("radio directory: {dir}"))),
        }
    }

    /// Never called (on-demand source); returns the favourites for tooling.
    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        if self.favourites.is_empty() {
            return Ok(Vec::new());
        }
        let found = self.stations_at(&path_by_uuids(&self.favourites)).await?;
        Ok(map_stations(
            &self.name,
            "favourites",
            &order_by_uuids(found, &self.favourites),
        ))
    }

    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let st = self
            .stations_at(&path_search(query, self.limit, self.hide_broken))
            .await?;
        Ok(map_stations(&self.name, "search", &st))
    }

    /// Resolve via `/json/url/{uuid}` (counts a click and returns the
    /// resolved stream URL), falling back to the station record.
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        if let Ok(body) = self
            .api_get(&format!("/json/url/{}", urlenc(song_id)))
            .await
            && let Some(url) = parse_click_url(&body)
        {
            return Ok(url);
        }
        self.station_by_uuid(song_id)
            .await?
            .and_then(|s| s.stream_url().map(str::to_owned))
            .ok_or_else(|| SourceError::NotFound(format!("radio station: {song_id}")))
    }

    /// Station favicon, when it has one.
    async fn cover_art(&self, song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        let Some(st) = self.station_by_uuid(song_id).await.ok().flatten() else {
            return Ok(None);
        };
        let Some(icon) = st
            .favicon
            .as_deref()
            .map(str::trim)
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        else {
            return Ok(None);
        };
        match send_bytes(self.http.get(icon)).await {
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
            .station_by_uuid(id)
            .await?
            .and_then(|s| map_station(&self.name, "stations", &s)))
    }

    fn is_live(&self, _song_id: &str) -> bool {
        true
    }

    fn sync_policy(&self) -> SyncPolicy {
        SyncPolicy::OnDemand
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const STATIONS: &str = r#"[
      {"changeuuid":"x","stationuuid":"aaaa-1111","name":" Jazz FM ","url":"http://jazz.example/live",
       "url_resolved":"http://cdn.example/jazz.aac","homepage":"https://jazz.example",
       "favicon":"https://jazz.example/i.png","tags":"jazz, smooth ,,lounge,extra","country":"United Kingdom",
       "countrycode":"GB","codec":"AAC+","bitrate":128,"votes":900,"lastcheckok":1},
      {"stationuuid":"bbbb-2222","name":"Plain","url":"http://plain.example/s","url_resolved":"",
       "tags":null,"codec":"UNKNOWN","bitrate":0,"favicon":null},
      {"stationuuid":"","name":"No uuid","url":"http://x"},
      {"stationuuid":"cccc-3333","name":"No url","url":"","url_resolved":null}
    ]"#;

    #[test]
    fn parses_stations_with_nulls() {
        let st = parse_stations(STATIONS.as_bytes()).unwrap();
        assert_eq!(st.len(), 4);
        assert_eq!(st[0].bitrate, Some(128));
        assert_eq!(st[1].tags, None);
        assert!(parse_stations(b"{}").is_err());
    }

    #[test]
    fn maps_station_to_live_song() {
        let st = parse_stations(STATIONS.as_bytes()).unwrap();
        let songs = map_stations("radio", "countries/United Kingdom", &st);
        // Stations without uuid or URL are dropped.
        assert_eq!(songs.len(), 2);
        let s = &songs[0];
        assert_eq!(
            s.path.as_str(),
            "radio/countries/United Kingdom/aaaa-1111.aac"
        );
        assert_eq!(s.duration, None);
        assert_eq!(s.bitrate, Some(128));
        let genres: Vec<&str> = s
            .tags
            .iter()
            .filter(|(k, _)| k == "genre")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(genres, vec!["jazz", "smooth", "lounge"]);
        assert!(s.tags.iter().any(|(k, v)| k == "title" && v == "Jazz FM"));
        assert!(s.tags.iter().any(|(k, v)| k == "name" && v == "Jazz FM"));
        // No known codec -> bare uuid leaf, and bitrate 0 is dropped.
        assert_eq!(
            songs[1].path.as_str(),
            "radio/countries/United Kingdom/bbbb-2222"
        );
        assert_eq!(songs[1].bitrate, None);
        assert_eq!(leaf_id(s.path.as_str()), "aaaa-1111");
    }

    #[test]
    fn stream_url_prefers_resolved() {
        let st = parse_stations(STATIONS.as_bytes()).unwrap();
        assert_eq!(st[0].stream_url(), Some("http://cdn.example/jazz.aac"));
        assert_eq!(st[1].stream_url(), Some("http://plain.example/s"));
        assert_eq!(st[3].stream_url(), None);
    }

    #[test]
    fn parses_countries_tags_and_servers() {
        let names = parse_names(
            br#"[{"name":"Germany","iso_3166_1":"DE","stationcount":5},{"name":" "},{"name":"France"}]"#,
        )
        .unwrap();
        assert_eq!(names, vec!["Germany", "France"]);
        let servers = parse_servers(
            br#"[{"ip":"1.2.3.4","name":"de1.api.radio-browser.info"},
                 {"ip":"1.2.3.5","name":"nl1.api.radio-browser.info"},
                 {"ip":"1.2.3.4","name":"de1.api.radio-browser.info"}]"#,
        );
        assert_eq!(
            servers,
            vec![
                "https://de1.api.radio-browser.info",
                "https://nl1.api.radio-browser.info"
            ]
        );
        assert!(parse_servers(b"garbage").is_empty());
    }

    #[test]
    fn parses_click_response() {
        let url = parse_click_url(
            br#"{"ok":"true","message":"retrieved station url","stationuuid":"a","name":"n","url":"http://s.example/x"}"#,
        );
        assert_eq!(url.as_deref(), Some("http://s.example/x"));
        assert_eq!(parse_click_url(br#"{"ok":false}"#), None);
    }

    #[test]
    fn favourites_follow_configured_order() {
        let st = parse_stations(STATIONS.as_bytes()).unwrap();
        let uuids = vec!["BBBB-2222".to_owned(), "aaaa-1111".to_owned()];
        let ordered = order_by_uuids(st[..2].to_vec(), &uuids);
        assert_eq!(ordered[0].uuid(), "bbbb-2222");
        assert_eq!(ordered[1].uuid(), "aaaa-1111");
    }

    #[test]
    fn api_paths() {
        assert_eq!(
            path_by_country("United Kingdom", 50, true),
            "/json/stations/bycountryexact/United%20Kingdom?hidebroken=true&order=votes&reverse=true&limit=50"
        );
        assert!(path_by_tag("drum & bass", 10, false).contains("bytagexact/drum%20%26%20bass?"));
        assert!(path_search("bbc 6", 100, true).starts_with("/json/stations/search?name=bbc%206&"));
        assert_eq!(
            path_by_uuids(&["a".to_owned(), "b".to_owned()]),
            "/json/stations/byuuid?uuids=a,b"
        );
    }

    fn cfg(entries: Vec<(&str, toml::Value)>) -> SourceConfig {
        let mut t = toml::Table::new();
        for (k, v) in entries {
            t.insert(k.to_owned(), v);
        }
        SourceConfig {
            name: "radio".to_owned(),
            source_type: "radio".to_owned(),
            enabled: true,
            settings: t,
        }
    }

    #[test]
    fn factory_settings() {
        let src = radio_source_factory(&cfg(vec![])).ok().unwrap();
        assert_eq!(src.scheme(), "radio");
        assert_eq!(src.sync_policy(), SyncPolicy::OnDemand);
        assert!(src.is_live("anything"));
        assert!(
            radio_source_factory(&cfg(vec![(
                "api_url",
                toml::Value::String("ftp://x".to_owned())
            )]))
            .is_err()
        );
        assert!(
            radio_source_factory(&cfg(vec![
                (
                    "api_url",
                    toml::Value::String("https://de1.example/".to_owned())
                ),
                (
                    "stations",
                    toml::Value::Array(vec![toml::Value::String("aaaa".to_owned())])
                ),
                ("limit", toml::Value::Integer(25)),
            ]))
            .is_ok()
        );
    }

    #[tokio::test]
    async fn browse_root_lists_sections_without_network() {
        let src = radio_source_factory(&cfg(vec![(
            "stations",
            toml::Value::Array(vec![toml::Value::String("aaaa".to_owned())]),
        )]))
        .ok()
        .unwrap();
        let dirs: Vec<String> = src
            .browse("")
            .await
            .ok()
            .unwrap()
            .into_iter()
            .filter_map(|e| match e {
                SourceEntry::Dir(d) => Some(d),
                SourceEntry::Song(_) => None,
            })
            .collect();
        assert_eq!(
            dirs,
            vec![
                "radio/favourites",
                "radio/top-voted",
                "radio/top-clicked",
                "radio/countries",
                "radio/tags"
            ]
        );
        assert!(src.browse("nonsense/a/b").await.is_err());
    }
}
