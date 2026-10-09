// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `PodcastSource` — podcasts from RSS feeds (with iTunes extensions).
//!
//! Compiled only when `feature = "podcast"` is active. [`SyncPolicy::OnDemand`]:
//! feeds are fetched when browsed (and cached for `cache_ttl` seconds).
//!
//! ```text
//! <name>/                          one directory per podcast (channel title)
//! <name>/<podcast>/<id>[.ext]      episodes, newest first
//! ```
//!
//! `feeds` lists feed URLs and/or OPML documents (a `.opml` path or URL whose
//! `<outline xmlUrl="…">` entries are expanded). An episode id is
//! `<16 hex: feed URL hash><16 hex: guid hash>`, which lets
//! [`MusicSource::resolve_stream_uri`] find the feed again from the id alone.
//! Feed URLs (which may embed private tokens) are never logged.

use crate::common::{
    dir_segments, enc, http_client, known_ext, leaf_id, make_song, mime_to_ext, send_bytes,
    setting_list,
};
use async_trait::async_trait;
use rmpd_core::config::SourceConfig;
use rmpd_core::song::Song;
use rmpd_plugin::source::{MusicSource, SourceEntry, SourceError, SourceResult, SyncPolicy};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Setting keys accepted in a `[[source]] type = "podcast"` block.
pub const SETTINGS: &[&str] = &["feeds", "max_episodes", "cache_ttl"];

const DEFAULT_MAX_EPISODES: usize = 50;
const DEFAULT_CACHE_TTL_SECS: u64 = 900;
/// Cap for `search` results.
const SEARCH_LIMIT: usize = 100;

// ─── Model ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Episode {
    id: String,
    title: String,
    url: String,
    mime: Option<String>,
    duration: Option<Duration>,
    /// Unix timestamp of `pubDate`.
    published: Option<i64>,
    image: Option<String>,
    author: Option<String>,
}

#[derive(Debug, Clone)]
struct Feed {
    #[cfg_attr(not(test), allow(dead_code))]
    id: String,
    title: String,
    author: Option<String>,
    image: Option<String>,
    /// Newest first.
    episodes: Vec<Episode>,
}

// ─── Hashing / ids ───────────────────────────────────────────────────────────

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 16 hex digits identifying a feed URL.
fn feed_id(url: &str) -> String {
    format!("{:016x}", fnv1a64(url.as_bytes()))
}

/// 32 hex digits: feed id + hash of the item key (guid or enclosure URL).
fn episode_id(feed_url: &str, key: &str) -> String {
    format!("{}{:016x}", feed_id(feed_url), fnv1a64(key.as_bytes()))
}

/// `true` for ids produced by [`episode_id`].
fn is_episode_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

// ─── Dates ───────────────────────────────────────────────────────────────────

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn month_num(s: &str) -> Option<i64> {
    let head: String = s.chars().take(3).collect::<String>().to_ascii_lowercase();
    Some(match head.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// Offset east of UTC in seconds for an RFC 2822 zone token (unknown → 0).
fn zone_offset_secs(zone: &str) -> i64 {
    let z = zone.trim();
    let named_hours: Option<i64> = match z.to_ascii_uppercase().as_str() {
        "EST" | "CDT" => Some(-5),
        "EDT" => Some(-4),
        "CST" | "MDT" => Some(-6),
        "MST" | "PDT" => Some(-7),
        "PST" => Some(-8),
        _ => None,
    };
    if let Some(h) = named_hours {
        return h * 3600;
    }
    let bytes = z.as_bytes();
    if bytes.len() == 5
        && (bytes[0] == b'+' || bytes[0] == b'-')
        && z[1..].bytes().all(|b| b.is_ascii_digit())
    {
        let hh: i64 = z[1..3].parse().unwrap_or(0);
        let mm: i64 = z[3..5].parse().unwrap_or(0);
        let secs = hh * 3600 + mm * 60;
        return if bytes[0] == b'-' { -secs } else { secs };
    }
    0
}

/// Parse an RFC 2822 date (`Tue, 10 Jun 2003 04:00:00 +0200`) to a Unix
/// timestamp. Tolerant: optional weekday, optional seconds/zone.
fn parse_rfc2822(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = match s.split_once(',') {
        Some((_, rest)) => rest.trim(),
        None => s,
    };
    let mut it = s.split_whitespace();
    let day: i64 = it.next()?.parse().ok()?;
    let month = month_num(it.next()?)?;
    let mut year: i64 = it.next()?.parse().ok()?;
    if year < 100 {
        year += if year < 50 { 2000 } else { 1900 };
    }
    let mut tp = it.next()?.split(':');
    let h: i64 = tp.next()?.parse().ok()?;
    let m: i64 = tp.next()?.parse().ok()?;
    let sec: i64 = tp.next().map_or(Some(0), |x| x.parse().ok())?;
    if !(1..=31).contains(&day)
        || !(0..24).contains(&h)
        || !(0..60).contains(&m)
        || !(0..=60).contains(&sec)
    {
        return None;
    }
    let offset = zone_offset_secs(it.next().unwrap_or("GMT"));
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + sec - offset)
}

/// `YYYY-MM-DD` of a Unix timestamp (UTC).
fn format_date(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// iTunes duration: `HH:MM:SS`, `MM:SS` or plain seconds.
fn parse_itunes_duration(s: &str) -> Option<Duration> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0f64;
    for p in parts {
        let v: f64 = p.trim().parse().ok()?;
        if !(0.0..1.0e7).contains(&v) {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    (secs > 0.0 && secs < 1.0e8).then(|| Duration::from_secs_f64(secs))
}

// ─── Parsing ─────────────────────────────────────────────────────────────────

/// Host part of a URL, used as a last-resort title.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Parse an RSS document into a [`Feed`] (episodes newest first, none dropped
/// except items without an enclosure).
fn parse_feed(feed_url: &str, body: &[u8]) -> SourceResult<Feed> {
    let channel = rss::Channel::read_from(body)
        .map_err(|e| SourceError::Protocol(format!("invalid RSS feed: {e}")))?;

    let itunes = channel.itunes_ext();
    let title = non_empty(Some(channel.title())).unwrap_or_else(|| host_of(feed_url));
    let author = itunes.and_then(|e| non_empty(e.author.as_deref()));
    let image = itunes
        .and_then(|e| non_empty(e.image.as_deref()))
        .or_else(|| {
            channel
                .image()
                .and_then(|i| non_empty(Some(i.url.as_str())))
        });

    let mut episodes: Vec<Episode> = Vec::new();
    for item in channel.items() {
        let Some(encl) = item.enclosure() else {
            continue;
        };
        let url = encl.url().trim();
        if url.is_empty() {
            continue;
        }
        let key = item
            .guid()
            .and_then(|g| non_empty(Some(g.value.as_str())))
            .unwrap_or_else(|| url.to_owned());
        let ext = item.itunes_ext();
        episodes.push(Episode {
            id: episode_id(feed_url, &key),
            title: non_empty(item.title()).unwrap_or_else(|| {
                url.rsplit('/')
                    .next()
                    .unwrap_or(url)
                    .split('?')
                    .next()
                    .unwrap_or(url)
                    .to_owned()
            }),
            url: url.to_owned(),
            mime: non_empty(Some(encl.mime_type())),
            duration: ext
                .and_then(|e| e.duration.as_deref())
                .and_then(parse_itunes_duration),
            published: item.pub_date().and_then(parse_rfc2822),
            image: ext.and_then(|e| non_empty(e.image.as_deref())),
            author: ext
                .and_then(|e| non_empty(e.author.as_deref()))
                .or_else(|| non_empty(item.author())),
        });
    }
    // Newest first; undated items keep their feed order at the end.
    episodes.sort_by_key(|e| std::cmp::Reverse(e.published.unwrap_or(i64::MIN)));

    Ok(Feed {
        id: feed_id(feed_url),
        title,
        author,
        image,
        episodes,
    })
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Value of attribute `name` (lowercase) inside one tag's text.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    while let Some(i) = lower[search..].find(name) {
        let at = search + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok && let Some(v) = rest.strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let inner = &v[1..];
                let end = inner.find(q)?;
                return Some(decode_entities(&inner[..end]));
            }
        }
        search = at + name.len();
    }
    None
}

/// Feed URLs (`xmlUrl` attributes of `<outline>` elements) of an OPML document.
fn extract_opml_urls(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<outline") {
        let start = pos + i;
        let Some(e) = lower[start..].find('>') else {
            break;
        };
        let end = start + e;
        if let Some(u) = attr_value(&xml[start..end], "xmlurl") {
            let u = u.trim().to_owned();
            if !u.is_empty() && !out.contains(&u) {
                out.push(u);
            }
        }
        pos = end;
    }
    out
}

fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// `feeds` entry that names an OPML document rather than a feed.
fn is_opml_ref(entry: &str) -> bool {
    let path = entry.split(['?', '#']).next().unwrap_or(entry);
    path.to_ascii_lowercase().ends_with(".opml") || !is_http_url(entry)
}

fn expand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    std::path::PathBuf::from(path)
}

// ─── Mapping ─────────────────────────────────────────────────────────────────

/// Unique per-feed directory names: duplicates get ` (2)`, ` (3)`, …
fn display_titles(feeds: &[Arc<Feed>]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    feeds
        .iter()
        .map(|f| {
            let n = seen.entry(f.title.clone()).or_insert(0);
            *n += 1;
            if *n == 1 {
                f.title.clone()
            } else {
                format!("{} ({})", f.title, *n)
            }
        })
        .collect()
}

/// File extension for the virtual leaf: from the MIME type, else the URL.
fn episode_ext(ep: &Episode) -> Option<String> {
    if let Some(e) = ep.mime.as_deref().and_then(mime_to_ext) {
        return Some(e.to_owned());
    }
    let path = ep.url.split(['?', '#']).next().unwrap_or(&ep.url);
    path.rsplit('/')
        .next()
        .and_then(|leaf| leaf.rsplit_once('.'))
        .and_then(|(_, ext)| known_ext(ext))
}

/// `<source>/<enc(dir_title)>/<id>[.ext]`.
fn map_episode(source: &str, dir_title: &str, feed: &Feed, ep: &Episode) -> Song {
    let leaf = match episode_ext(ep) {
        Some(e) => format!("{}.{}", ep.id, e),
        None => ep.id.clone(),
    };
    let path = format!("{}/{}/{}", source, enc(dir_title), leaf);
    let artist = ep
        .author
        .clone()
        .or_else(|| feed.author.clone())
        .unwrap_or_else(|| feed.title.clone());
    let tags: Vec<(&str, String)> = vec![
        ("title", ep.title.clone()),
        ("artist", artist.clone()),
        ("albumartist", artist),
        ("album", feed.title.clone()),
        ("date", ep.published.map(format_date).unwrap_or_default()),
        ("genre", "Podcast".to_owned()),
    ];
    make_song(path, tags, ep.duration, None)
}

// ─── Source ──────────────────────────────────────────────────────────────────

/// Podcast source backed by RSS feeds.
pub struct PodcastSource {
    name: String,
    http: reqwest::Client,
    entries: Vec<String>,
    max_episodes: usize,
    ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, Arc<Feed>)>>,
    expanded: Mutex<Option<(Instant, Vec<String>)>>,
}

/// Sync, no-I/O factory registered in `SOURCE_PLUGINS` under `feature = "podcast"`.
pub fn podcast_source_factory(cfg: &SourceConfig) -> Result<Box<dyn MusicSource>, SourceError> {
    let entries = setting_list(cfg, "feeds");
    if entries.is_empty() {
        return Err(SourceError::Config(
            "podcast source requires a `feeds` setting (feed URLs and/or an OPML file)".to_owned(),
        ));
    }
    let max_episodes = cfg
        .setting_str("max_episodes")
        .and_then(|s| s.parse::<usize>().ok())
        .map_or(DEFAULT_MAX_EPISODES, |n| n.max(1));
    let ttl = cfg
        .setting_str("cache_ttl")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CACHE_TTL_SECS);
    Ok(Box::new(PodcastSource {
        name: cfg.name.clone(),
        http: http_client(Duration::from_secs(30), false)?,
        entries,
        max_episodes,
        ttl: Duration::from_secs(ttl),
        cache: Mutex::new(HashMap::new()),
        expanded: Mutex::new(None),
    }))
}

impl PodcastSource {
    /// Feed URLs after OPML expansion (cached for the TTL).
    async fn feed_urls(&self) -> Vec<String> {
        {
            let guard = self.expanded.lock().await;
            if let Some((at, urls)) = guard.as_ref()
                && at.elapsed() < self.ttl
            {
                return urls.clone();
            }
        }
        let mut urls: Vec<String> = Vec::new();
        for entry in &self.entries {
            if !is_opml_ref(entry) {
                if !urls.contains(entry) {
                    urls.push(entry.clone());
                }
                continue;
            }
            let content = if is_http_url(entry) {
                send_bytes(self.http.get(entry.as_str()))
                    .await
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .map_err(|e| e.to_string())
            } else {
                tokio::fs::read_to_string(expand_home(entry))
                    .await
                    .map_err(|e| e.to_string())
            };
            match content {
                Ok(xml) => {
                    for u in extract_opml_urls(&xml) {
                        if !urls.contains(&u) {
                            urls.push(u);
                        }
                    }
                }
                Err(e) => tracing::warn!("podcast source '{}': cannot read OPML ({e})", self.name),
            }
        }
        *self.expanded.lock().await = Some((Instant::now(), urls.clone()));
        urls
    }

    /// Fetch (or reuse) one feed. A stale cache entry is served when the
    /// refresh fails.
    async fn load_feed(&self, url: &str) -> SourceResult<Arc<Feed>> {
        let stale = {
            let cache = self.cache.lock().await;
            match cache.get(url) {
                Some((at, feed)) if at.elapsed() < self.ttl => return Ok(Arc::clone(feed)),
                Some((_, feed)) => Some(Arc::clone(feed)),
                None => None,
            }
        };
        let fetched = async {
            let body = send_bytes(self.http.get(url).header(
                "Accept",
                "application/rss+xml, application/xml, text/xml, */*",
            ))
            .await?;
            parse_feed(url, &body)
        }
        .await;
        match fetched {
            Ok(feed) => {
                let feed = Arc::new(feed);
                self.cache
                    .lock()
                    .await
                    .insert(url.to_owned(), (Instant::now(), Arc::clone(&feed)));
                Ok(feed)
            }
            Err(e) => stale.ok_or(e),
        }
    }

    /// Every feed that loads; failures are logged by feed id only.
    async fn load_all(&self) -> Vec<Arc<Feed>> {
        let urls = self.feed_urls().await;
        let results = futures::future::join_all(urls.iter().map(|u| self.load_feed(u))).await;
        let mut feeds = Vec::new();
        for (url, res) in urls.iter().zip(results) {
            match res {
                Ok(f) => feeds.push(f),
                Err(e) => tracing::warn!(
                    "podcast source '{}': feed {} failed ({e})",
                    self.name,
                    feed_id(url)
                ),
            }
        }
        feeds
    }

    /// Locate an episode by id (its feed id prefix selects the feed).
    async fn find_episode(&self, id: &str) -> SourceResult<(Arc<Feed>, Episode)> {
        if !is_episode_id(id) {
            return Err(SourceError::NotFound(format!("podcast episode: {id}")));
        }
        let urls = self.feed_urls().await;
        let url = urls
            .iter()
            .find(|u| feed_id(u) == id[..16])
            .ok_or_else(|| SourceError::NotFound(format!("podcast episode: {id}")))?;
        let feed = self.load_feed(url).await?;
        let ep = feed
            .episodes
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| SourceError::NotFound(format!("podcast episode: {id}")))?;
        Ok((feed, ep))
    }
}

#[async_trait]
impl MusicSource for PodcastSource {
    fn scheme(&self) -> &str {
        "podcast"
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn ping(&self) -> SourceResult<()> {
        let urls = self.feed_urls().await;
        if urls.is_empty() {
            return Err(SourceError::Config(
                "podcast source has no feeds".to_owned(),
            ));
        }
        if self.load_all().await.is_empty() {
            return Err(SourceError::Unreachable(
                "no podcast feed loaded".to_owned(),
            ));
        }
        Ok(())
    }

    async fn browse(&self, dir: &str) -> SourceResult<Vec<SourceEntry>> {
        let segs = dir_segments(dir);
        let feeds = self.load_all().await;
        let titles = display_titles(&feeds);
        match segs.as_slice() {
            [] => Ok(titles
                .iter()
                .map(|t| SourceEntry::Dir(format!("{}/{}", self.name, enc(t))))
                .collect()),
            [podcast] => {
                let idx = titles
                    .iter()
                    .position(|t| t == podcast)
                    .ok_or_else(|| SourceError::NotFound(format!("podcast: {podcast}")))?;
                let feed = &feeds[idx];
                Ok(feed
                    .episodes
                    .iter()
                    .take(self.max_episodes)
                    .map(|ep| SourceEntry::Song(map_episode(&self.name, &titles[idx], feed, ep)))
                    .collect())
            }
            _ => Err(SourceError::NotFound(format!("podcast directory: {dir}"))),
        }
    }

    /// Never called (on-demand source); returns the newest episodes per feed.
    async fn list_all(&self) -> SourceResult<Vec<Song>> {
        let feeds = self.load_all().await;
        let titles = display_titles(&feeds);
        let mut out = Vec::new();
        for (feed, title) in feeds.iter().zip(&titles) {
            for ep in feed.episodes.iter().take(self.max_episodes) {
                out.push(map_episode(&self.name, title, feed, ep));
            }
        }
        Ok(out)
    }

    async fn search(&self, query: &str) -> SourceResult<Vec<Song>> {
        let q = query.trim().to_lowercase();
        let feeds = self.load_all().await;
        let titles = display_titles(&feeds);
        let mut out = Vec::new();
        'outer: for (feed, title) in feeds.iter().zip(&titles) {
            for ep in &feed.episodes {
                if ep.title.to_lowercase().contains(&q) || feed.title.to_lowercase().contains(&q) {
                    out.push(map_episode(&self.name, title, feed, ep));
                    if out.len() >= SEARCH_LIMIT {
                        break 'outer;
                    }
                }
            }
        }
        Ok(out)
    }

    /// The episode's enclosure URL.
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        Ok(self.find_episode(song_id).await?.1.url)
    }

    /// Episode image, else the podcast's (`itunes:image`).
    async fn cover_art(&self, song_id: &str) -> SourceResult<Option<Vec<u8>>> {
        let Ok((feed, ep)) = self.find_episode(song_id).await else {
            return Ok(None);
        };
        let Some(url) = ep.image.or(feed.image.clone()).filter(|u| is_http_url(u)) else {
            return Ok(None);
        };
        match send_bytes(self.http.get(url.as_str())).await {
            Ok(b) if !b.is_empty() => Ok(Some(b)),
            _ => Ok(None),
        }
    }

    async fn lookup(&self, uri: &str) -> SourceResult<Option<Song>> {
        match self.find_episode(leaf_id(uri)).await {
            Ok((feed, ep)) => Ok(Some(map_episode(&self.name, &feed.title, &feed, &ep))),
            Err(SourceError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn sync_policy(&self) -> SyncPolicy {
        SyncPolicy::OnDemand
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const FEED_URL: &str = "https://pod.example/feed.xml";

    const RSS_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title> The Test Cast </title>
    <link>https://pod.example</link>
    <description>desc</description>
    <itunes:author>Test Author</itunes:author>
    <itunes:image href="https://pod.example/cover.jpg"/>
    <item>
      <title>Episode 1</title>
      <guid isPermaLink="false">ep-1</guid>
      <pubDate>Mon, 01 Jan 2024 08:00:00 GMT</pubDate>
      <enclosure url="https://cdn.example/e1.mp3?token=abc" length="123" type="audio/mpeg"/>
      <itunes:duration>1:02:03</itunes:duration>
    </item>
    <item>
      <title>Episode 3</title>
      <guid>ep-3</guid>
      <pubDate>Wed, 15 May 2024 10:30:00 +0200</pubDate>
      <enclosure url="https://cdn.example/e3.m4a" length="1" type="audio/x-m4a"/>
      <itunes:duration>754</itunes:duration>
      <itunes:author>Guest</itunes:author>
    </item>
    <item>
      <title>No audio here</title>
      <pubDate>Tue, 02 Jan 2024 08:00:00 GMT</pubDate>
    </item>
    <item>
      <title>Episode 2</title>
      <pubDate>Tue, 06 Feb 2024 08:00:00 GMT</pubDate>
      <enclosure url="https://cdn.example/e2.ogg" length="1" type=""/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_feed_metadata_and_orders_newest_first() {
        let feed = parse_feed(FEED_URL, RSS_FIXTURE.as_bytes()).unwrap();
        assert_eq!(feed.title, "The Test Cast");
        assert_eq!(feed.author.as_deref(), Some("Test Author"));
        assert_eq!(feed.image.as_deref(), Some("https://pod.example/cover.jpg"));
        let titles: Vec<&str> = feed.episodes.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Episode 3", "Episode 2", "Episode 1"]);
        assert_eq!(feed.id, feed_id(FEED_URL));
    }

    #[test]
    fn parses_episode_fields() {
        let feed = parse_feed(FEED_URL, RSS_FIXTURE.as_bytes()).unwrap();
        let e1 = feed
            .episodes
            .iter()
            .find(|e| e.title == "Episode 1")
            .unwrap();
        assert_eq!(e1.url, "https://cdn.example/e1.mp3?token=abc");
        assert_eq!(e1.duration, Some(Duration::from_secs(3723)));
        assert_eq!(e1.published, Some(1_704_096_000));
        assert_eq!(e1.id, episode_id(FEED_URL, "ep-1"));
        assert!(is_episode_id(&e1.id));
        let e3 = &feed.episodes[0];
        assert_eq!(e3.duration, Some(Duration::from_secs(754)));
        assert_eq!(e3.author.as_deref(), Some("Guest"));
        // 10:30 +0200 == 08:30 UTC
        assert_eq!(e3.published, parse_rfc2822("Wed, 15 May 2024 08:30:00 GMT"));
        // Without guid the enclosure URL is the key.
        let e2 = &feed.episodes[1];
        assert_eq!(e2.id, episode_id(FEED_URL, "https://cdn.example/e2.ogg"));
    }

    #[test]
    fn rejects_non_rss() {
        assert!(parse_feed(FEED_URL, b"<html><body>nope</body></html>").is_err());
        assert!(parse_feed(FEED_URL, b"").is_err());
    }

    #[test]
    fn maps_episode_to_song() {
        let feed = parse_feed(FEED_URL, RSS_FIXTURE.as_bytes()).unwrap();
        let e1 = feed
            .episodes
            .iter()
            .find(|e| e.title == "Episode 1")
            .unwrap();
        let s = map_episode("pods", &feed.title, &feed, e1);
        assert_eq!(s.path.as_str(), format!("pods/The Test Cast/{}.mp3", e1.id));
        assert_eq!(s.duration, Some(Duration::from_secs(3723)));
        let tag = |k: &str| {
            s.tags
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(tag("title"), Some("Episode 1"));
        assert_eq!(tag("album"), Some("The Test Cast"));
        assert_eq!(tag("artist"), Some("Test Author"));
        assert_eq!(tag("date"), Some("2024-01-01"));
        assert_eq!(leaf_id(s.path.as_str()), e1.id);
        // ogg extension derived from the URL when the MIME type is empty.
        let e2 = &feed.episodes[1];
        assert!(
            map_episode("pods", "x/y", &feed, e2)
                .path
                .as_str()
                .ends_with(".ogg")
        );
        assert!(
            map_episode("pods", "x/y", &feed, e2)
                .path
                .as_str()
                .starts_with("pods/x%2Fy/")
        );
    }

    #[test]
    fn rfc2822_dates() {
        assert_eq!(parse_rfc2822("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            parse_rfc2822("Tue, 10 Jun 2003 04:00:00 GMT"),
            Some(1_055_217_600)
        );
        assert_eq!(
            parse_rfc2822("Tue, 10 Jun 2003 06:00:00 +0200"),
            Some(1_055_217_600)
        );
        assert_eq!(
            parse_rfc2822("10 Jun 03 00:00 EST"),
            Some(1_055_217_600 - 4 * 3600 + 5 * 3600)
        );
        assert_eq!(parse_rfc2822("garbage"), None);
        assert_eq!(parse_rfc2822("Tue, 32 Jun 2003 04:00:00 GMT"), None);
        assert_eq!(format_date(1_055_217_600), "2003-06-10");
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_709_164_800), "2024-02-29");
    }

    #[test]
    fn itunes_durations() {
        assert_eq!(
            parse_itunes_duration("1:02:03"),
            Some(Duration::from_secs(3723))
        );
        assert_eq!(
            parse_itunes_duration("12:34"),
            Some(Duration::from_secs(754))
        );
        assert_eq!(parse_itunes_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_itunes_duration("0"), None);
        assert_eq!(parse_itunes_duration("abc"), None);
        assert_eq!(parse_itunes_duration("1:2:3:4"), None);
    }

    #[test]
    fn opml_urls_extracted() {
        let opml = r#"<?xml version="1.0"?>
<opml version="2.0"><head><title>subs</title></head><body>
  <outline text="Tech" title="Tech">
    <outline type="rss" text="A" xmlUrl="https://a.example/feed?x=1&amp;y=2" htmlUrl="https://a.example"/>
    <outline type="rss" text="B" xmlurl='https://b.example/rss'/>
    <outline type="rss" text="dup" xmlUrl="https://b.example/rss"></outline>
  </outline>
  <outline text="no url"/>
</body></opml>"#;
        assert_eq!(
            extract_opml_urls(opml),
            vec!["https://a.example/feed?x=1&y=2", "https://b.example/rss"]
        );
        assert!(extract_opml_urls("<opml></opml>").is_empty());
    }

    #[test]
    fn feed_entry_classification() {
        assert!(!is_opml_ref("https://x.example/feed.xml"));
        assert!(is_opml_ref("https://x.example/subs.opml?dl=1"));
        assert!(is_opml_ref("/home/me/subs.opml"));
        assert!(is_opml_ref("~/podcasts.xml"));
    }

    #[test]
    fn unique_display_titles() {
        let mk = |t: &str| {
            Arc::new(Feed {
                id: String::new(),
                title: t.to_owned(),
                author: None,
                image: None,
                episodes: Vec::new(),
            })
        };
        let feeds = vec![mk("A"), mk("B"), mk("A")];
        assert_eq!(display_titles(&feeds), vec!["A", "B", "A (2)"]);
    }

    #[test]
    fn host_fallback_title() {
        assert_eq!(
            host_of("https://user:pw@pod.example:8080/x?y#z"),
            "pod.example:8080"
        );
        let feed = parse_feed(
            "https://pod.example/f.xml",
            br#"<rss version="2.0"><channel><title></title><link>l</link><description>d</description></channel></rss>"#,
        )
        .unwrap();
        assert_eq!(feed.title, "pod.example");
    }

    fn cfg(entries: Vec<(&str, toml::Value)>) -> SourceConfig {
        let mut t = toml::Table::new();
        for (k, v) in entries {
            t.insert(k.to_owned(), v);
        }
        SourceConfig {
            name: "pods".to_owned(),
            source_type: "podcast".to_owned(),
            enabled: true,
            settings: t,
        }
    }

    #[test]
    fn factory_requires_feeds() {
        assert!(podcast_source_factory(&cfg(vec![])).is_err());
        let src = podcast_source_factory(&cfg(vec![(
            "feeds",
            toml::Value::Array(vec![toml::Value::String(FEED_URL.to_owned())]),
        )]))
        .ok()
        .unwrap();
        assert_eq!(src.scheme(), "podcast");
        assert_eq!(src.name(), "pods");
        assert_eq!(src.sync_policy(), SyncPolicy::OnDemand);
        assert!(!src.is_live("x"));
    }

    #[tokio::test]
    async fn unknown_episode_id_is_not_found_without_network() {
        let src = podcast_source_factory(&cfg(vec![(
            "feeds",
            toml::Value::String(FEED_URL.to_owned()),
        )]))
        .ok()
        .unwrap();
        assert!(matches!(
            src.resolve_stream_uri("not-an-id").await,
            Err(SourceError::NotFound(_))
        ));
        // Valid shape but no configured feed hashes to it.
        let bogus = "0".repeat(32);
        assert!(matches!(
            src.resolve_stream_uri(&bogus).await,
            Err(SourceError::NotFound(_))
        ));
    }
}
