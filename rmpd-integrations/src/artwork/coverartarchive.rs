// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cover Art Archive artwork provider (`type = "coverartarchive"`).
//!
//! Resolution order for a song:
//!
//! 1. `MUSICBRAINZ_ALBUMID` tag → `<caa_url>/release/<mbid>/front-<size>`.
//! 2. `MUSICBRAINZ_RELEASEGROUPID` tag → `<caa_url>/release-group/<mbid>/front-<size>`
//!    (when the release itself has no front cover).
//! 3. With `lookup = true` and no usable album MBID: a MusicBrainz release
//!    search by album artist + album, then step 1 with the best match.
//!    MusicBrainz requests are rate limited to one per second and carry a
//!    descriptive `User-Agent`, as the MusicBrainz API terms require.
//!
//! The provider is stateless apart from its HTTP client and the MusicBrainz
//! rate limiter; caching (including negative caching) is done by the caller.

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use rmpd_core::config::ArtworkConfig;
use rmpd_core::song::Song;
use rmpd_plugin::{ArtworkOutcome, ArtworkPlugin, ArtworkProvider, PluginError};
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Settings accepted by this provider.
pub const SETTINGS: &[&str] = &["lookup", "size", "contact", "caa_url", "mb_url"];

/// Registry entry.
pub const PLUGIN: ArtworkPlugin = ArtworkPlugin {
    name: "coverartarchive",
    settings: SETTINGS,
    factory,
};

const DEFAULT_CAA_URL: &str = "https://coverartarchive.org";
const DEFAULT_MB_URL: &str = "https://musicbrainz.org";
const DEFAULT_SIZE: &str = "500";
const DEFAULT_CONTACT: &str = "https://github.com/M0Rf30/rmpd";
/// Thumbnail sizes the Cover Art Archive serves, plus the original image.
const SIZES: &[&str] = &["250", "500", "1200", "original"];
/// Never download more than this for one image (matches the artwork cache cap).
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Cap on a MusicBrainz search response.
const MAX_SEARCH_BYTES: usize = 1024 * 1024;
/// MusicBrainz allows at most one request per second per client.
const MB_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Minimum MusicBrainz search score (0-100) to accept a release.
const MIN_SEARCH_SCORE: u32 = 90;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Pure helpers ────────────────────────────────────────────────────────────

/// Validate a MusicBrainz ID (UUID, 8-4-4-4-12 hex) and return it lowercased.
#[must_use]
pub fn parse_mbid(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() != 36 {
        return None;
    }
    let ok = b.iter().enumerate().all(|(i, c)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            *c == b'-'
        } else {
            c.is_ascii_hexdigit()
        }
    });
    ok.then(|| s.to_ascii_lowercase())
}

/// Cover Art Archive URL of the front image of `entity` (`release` or
/// `release-group`). `size` is `250`/`500`/`1200` or `original`.
#[must_use]
pub fn cover_url(base: &str, entity: &str, mbid: &str, size: &str) -> String {
    let file = if size == "original" {
        "front".to_owned()
    } else {
        format!("front-{size}")
    };
    format!("{}/{entity}/{mbid}/{file}", base.trim_end_matches('/'))
}

/// Quote `s` as a Lucene phrase (escaping `\` and `"`).
fn lucene_phrase(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(c, '\\' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// MusicBrainz release-search URL for an album artist + album pair.
#[must_use]
pub fn search_url(mb_base: &str, artist: &str, album: &str) -> Option<String> {
    let mut url =
        reqwest::Url::parse(&format!("{}/ws/2/release/", mb_base.trim_end_matches('/'))).ok()?;
    let query = format!(
        "release:{} AND artist:{}",
        lucene_phrase(album),
        lucene_phrase(artist)
    );
    url.query_pairs_mut()
        .append_pair("query", &query)
        .append_pair("fmt", "json")
        .append_pair("limit", "5");
    Some(url.into())
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    releases: Vec<SearchRelease>,
}

#[derive(Deserialize)]
struct SearchRelease {
    id: String,
    /// MusicBrainz sends a number; older servers sent a numeric string.
    #[serde(default)]
    score: Option<serde_json::Value>,
}

fn score_of(v: Option<&serde_json::Value>) -> u32 {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map_or(0, |x| u32::try_from(x).unwrap_or(u32::MAX)),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Pick the best release MBID from a MusicBrainz search response: the highest
/// score at or above [`MIN_SEARCH_SCORE`] (first wins on ties) with a valid ID.
#[must_use]
pub fn parse_search_response(body: &[u8]) -> Option<String> {
    let resp: SearchResponse = serde_json::from_slice(body).ok()?;
    let mut best: Option<(u32, String)> = None;
    for rel in &resp.releases {
        let score = score_of(rel.score.as_ref());
        if score < MIN_SEARCH_SCORE {
            continue;
        }
        let Some(id) = parse_mbid(&rel.id) else {
            continue;
        };
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id)
}

/// How long to wait before the next MusicBrainz request: `interval` after the
/// previous one, measured from `last`.
#[must_use]
pub fn slot_delay(last: Option<Instant>, now: Instant, interval: Duration) -> Duration {
    match last {
        Some(l) => (l + interval).saturating_duration_since(now),
        None => Duration::ZERO,
    }
}

/// `User-Agent` for MusicBrainz / Cover Art Archive: product, version and a
/// contact (their API terms require a way to reach the client's operator).
#[must_use]
pub fn user_agent(contact: Option<&str>) -> String {
    format!(
        "rmpd/{} ( {} )",
        env!("CARGO_PKG_VERSION"),
        contact.unwrap_or(DEFAULT_CONTACT)
    )
}

/// Identify an image by its magic bytes.
fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// MIME type of a downloaded body: magic bytes first (an HTML error page
/// served with a 200 must not be cached as art), else an `image/*`
/// `Content-Type`. `None` means "not an image".
#[must_use]
pub fn resolve_mime(content_type: Option<&str>, data: &[u8]) -> Option<String> {
    if let Some(m) = sniff_mime(data) {
        return Some(m.to_owned());
    }
    content_type
        .map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|c| c.starts_with("image/") && c.len() > "image/".len())
}

// ─── Rate limiter ────────────────────────────────────────────────────────────

#[derive(Debug)]
struct RateLimiter {
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(None),
        }
    }

    /// Wait for this caller's slot. The lock is held while sleeping so
    /// concurrent callers queue up one interval apart.
    async fn acquire(&self) {
        let mut last = self.last.lock().await;
        let now = Instant::now();
        let delay = slot_delay(*last, now, self.interval);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        *last = Some(now + delay);
    }
}

// ─── Provider ────────────────────────────────────────────────────────────────

/// Why a request produced no body.
enum Failure {
    /// The server answered: nothing there (404, other client error, not an
    /// image, too large).
    Missing,
    /// Could not get an answer (network, timeout, 429/5xx).
    Transient,
}

/// Cover Art Archive provider.
pub struct CoverArtArchive {
    name: String,
    lookup: bool,
    size: String,
    caa_url: String,
    mb_url: String,
    user_agent: String,
    client: OnceLock<Option<reqwest::Client>>,
    mb_limiter: RateLimiter,
}

impl std::fmt::Debug for CoverArtArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoverArtArchive")
            .field("name", &self.name)
            .field("lookup", &self.lookup)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

fn base_url(cfg: &ArtworkConfig, key: &str, default: &str) -> Result<String, PluginError> {
    let url = cfg
        .setting_str(key)
        .unwrap_or_else(|| default.to_owned())
        .trim_end_matches('/')
        .to_owned();
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url)
    } else {
        Err(PluginError::Config(format!(
            "artwork `{}`: `{key}` must be an http(s) URL",
            cfg.name
        )))
    }
}

impl CoverArtArchive {
    /// Validate settings and build the provider (no I/O).
    pub fn from_config(cfg: &ArtworkConfig) -> Result<Self, PluginError> {
        let size = cfg
            .setting_str("size")
            .unwrap_or_else(|| DEFAULT_SIZE.to_owned())
            .to_ascii_lowercase();
        if !SIZES.contains(&size.as_str()) {
            return Err(PluginError::Config(format!(
                "artwork `{}`: `size` must be one of {}",
                cfg.name,
                SIZES.join(", ")
            )));
        }
        let ua = user_agent(cfg.setting_str("contact").as_deref());
        if HeaderValue::from_str(&ua).is_err() {
            return Err(PluginError::Config(format!(
                "artwork `{}`: `contact` contains characters not allowed in a User-Agent",
                cfg.name
            )));
        }
        Ok(Self {
            name: cfg.name.clone(),
            lookup: cfg.setting_bool("lookup", false),
            size,
            caa_url: base_url(cfg, "caa_url", DEFAULT_CAA_URL)?,
            mb_url: base_url(cfg, "mb_url", DEFAULT_MB_URL)?,
            user_agent: ua,
            client: OnceLock::new(),
            mb_limiter: RateLimiter::new(MB_MIN_INTERVAL),
        })
    }

    fn client(&self) -> Option<&reqwest::Client> {
        self.client
            .get_or_init(|| {
                reqwest::Client::builder()
                    .user_agent(self.user_agent.as_str())
                    .timeout(REQUEST_TIMEOUT)
                    .connect_timeout(CONNECT_TIMEOUT)
                    .build()
                    .map_err(|e| {
                        tracing::warn!(
                            name = %self.name,
                            "cannot build HTTP client: {}",
                            e.without_url()
                        );
                    })
                    .ok()
            })
            .as_ref()
    }

    /// GET `url`, returning the body (capped at `limit`) and its
    /// `Content-Type`.
    async fn get(&self, url: &str, limit: usize) -> Result<(Vec<u8>, Option<String>), Failure> {
        let client = self.client().ok_or(Failure::Transient)?;
        let mut resp = client.get(url).send().await.map_err(|e| {
            tracing::debug!(name = %self.name, "artwork request failed: {}", e.without_url());
            Failure::Transient
        })?;
        let status = resp.status();
        if !status.is_success() {
            tracing::debug!(name = %self.name, %status, "artwork request not successful");
            let transient = status.is_server_error()
                || status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::REQUEST_TIMEOUT;
            return Err(if transient {
                Failure::Transient
            } else {
                Failure::Missing
            });
        }
        if resp
            .content_length()
            .is_some_and(|l| l > u64::try_from(limit).unwrap_or(u64::MAX))
        {
            return Err(Failure::Missing);
        }
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > limit {
                        return Err(Failure::Missing);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(
                        name = %self.name,
                        "artwork download failed: {}",
                        e.without_url()
                    );
                    return Err(Failure::Transient);
                }
            }
        }
        Ok((body, content_type))
    }

    /// Try the release, then the release group, for a front cover.
    async fn fetch_front(&self, release: Option<&str>, group: Option<&str>) -> ArtworkOutcome {
        let mut transient = false;
        for (entity, id) in [("release", release), ("release-group", group)] {
            let Some(id) = id else { continue };
            let url = cover_url(&self.caa_url, entity, id, &self.size);
            match self.get(&url, MAX_IMAGE_BYTES).await {
                Ok((data, content_type)) => {
                    if let Some(mime) = resolve_mime(content_type.as_deref(), &data) {
                        return ArtworkOutcome::Found(data, mime);
                    }
                }
                Err(Failure::Missing) => {}
                Err(Failure::Transient) => transient = true,
            }
        }
        if transient {
            ArtworkOutcome::Unavailable
        } else {
            ArtworkOutcome::NotFound
        }
    }

    /// MusicBrainz search for the release MBID of `artist` + `album`.
    async fn search_release(&self, artist: &str, album: &str) -> Result<Option<String>, Failure> {
        let Some(url) = search_url(&self.mb_url, artist, album) else {
            return Ok(None);
        };
        self.mb_limiter.acquire().await;
        match self.get(&url, MAX_SEARCH_BYTES).await {
            Ok((body, _)) => Ok(parse_search_response(&body)),
            Err(Failure::Missing) => Ok(None),
            Err(Failure::Transient) => Err(Failure::Transient),
        }
    }
}

fn tag_mbid(song: &Song, tag: &str) -> Option<String> {
    song.tag(tag).and_then(parse_mbid)
}

fn tag_text(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|s| !s.is_empty())
}

#[async_trait]
impl ArtworkProvider for CoverArtArchive {
    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch(&self, song: &Song) -> Option<(Vec<u8>, String)> {
        match self.fetch_outcome(song).await {
            ArtworkOutcome::Found(data, mime) => Some((data, mime)),
            _ => None,
        }
    }

    async fn fetch_outcome(&self, song: &Song) -> ArtworkOutcome {
        let group = tag_mbid(song, "musicbrainz_releasegroupid");
        let release = match tag_mbid(song, "musicbrainz_albumid") {
            Some(id) => Some(id),
            None if self.lookup => {
                let album = tag_text(song.tag("album"));
                let artist = tag_text(song.tag_with_fallback("albumartist"));
                let (Some(album), Some(artist)) = (album, artist) else {
                    return ArtworkOutcome::NotFound;
                };
                match self.search_release(artist, album).await {
                    Ok(found) => found,
                    Err(_) => return ArtworkOutcome::Unavailable,
                }
            }
            None => None,
        };
        if release.is_none() && group.is_none() {
            return ArtworkOutcome::NotFound;
        }
        self.fetch_front(release.as_deref(), group.as_deref()).await
    }
}

fn factory(cfg: &ArtworkConfig) -> Result<Box<dyn ArtworkProvider>, PluginError> {
    Ok(Box::new(CoverArtArchive::from_config(cfg)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::intern_tag_key;

    const MBID: &str = "76df3287-6cda-33eb-8e9a-044b5e15ffdd";

    fn cfg(entries: &[(&str, toml::Value)]) -> ArtworkConfig {
        let mut settings = toml::Table::new();
        for (k, v) in entries {
            settings.insert((*k).to_owned(), v.clone());
        }
        ArtworkConfig {
            name: "caa".to_owned(),
            artwork_type: "coverartarchive".to_owned(),
            enabled: true,
            settings,
        }
    }

    fn song(tags: &[(&str, &str)]) -> Song {
        Song {
            id: 1,
            path: "a/b.flac".into(),
            duration: None,
            sample_rate: None,
            channels: None,
            bits_per_sample: None,
            bitrate: None,
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
            range: None,
            tags: tags
                .iter()
                .map(|(k, v)| (intern_tag_key(k), (*v).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn mbid_validation() {
        assert_eq!(parse_mbid(MBID).as_deref(), Some(MBID));
        assert_eq!(
            parse_mbid(&format!("  {}  ", MBID.to_uppercase())).as_deref(),
            Some(MBID)
        );
        assert_eq!(parse_mbid(""), None);
        assert_eq!(parse_mbid("not-a-uuid"), None);
        assert_eq!(parse_mbid("76df3287-6cda-33eb-8e9a-044b5e15ffd"), None);
        assert_eq!(parse_mbid("76df3287x6cda-33eb-8e9a-044b5e15ffdd"), None);
        assert_eq!(parse_mbid("g6df3287-6cda-33eb-8e9a-044b5e15ffdd"), None);
        // Path traversal / URL injection through a tag must not validate.
        assert_eq!(parse_mbid("../../etc/passwd"), None);
    }

    #[test]
    fn cover_url_building() {
        assert_eq!(
            cover_url("https://coverartarchive.org", "release", MBID, "500"),
            format!("https://coverartarchive.org/release/{MBID}/front-500")
        );
        assert_eq!(
            cover_url("https://caa.example/", "release-group", MBID, "1200"),
            format!("https://caa.example/release-group/{MBID}/front-1200")
        );
        assert_eq!(
            cover_url("https://caa.example", "release", MBID, "original"),
            format!("https://caa.example/release/{MBID}/front")
        );
    }

    #[test]
    fn search_url_building_escapes_query() {
        let u = search_url("https://musicbrainz.org/", "AC/DC", "Back \"in\" Black").unwrap();
        let url = reqwest::Url::parse(&u).unwrap();
        assert_eq!(url.path(), "/ws/2/release/");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("query"),
            Some(r#"release:"Back \"in\" Black" AND artist:"AC/DC""#)
        );
        assert_eq!(get("fmt"), Some("json"));
        assert_eq!(get("limit"), Some("5"));
        assert!(search_url("not a url", "a", "b").is_none());
    }

    #[test]
    fn search_response_parsing() {
        let body = format!(
            r#"{{"created":"x","count":3,"releases":[
                {{"id":"{MBID}","score":100,"title":"A"}},
                {{"id":"11111111-1111-1111-1111-111111111111","score":95}},
                {{"id":"22222222-2222-2222-2222-222222222222","score":100}}
            ]}}"#
        );
        // Highest score, first wins on a tie.
        assert_eq!(
            parse_search_response(body.as_bytes()).as_deref(),
            Some(MBID)
        );
    }

    #[test]
    fn search_response_prefers_higher_score_and_accepts_string_scores() {
        let body = r#"{"releases":[
            {"id":"11111111-1111-1111-1111-111111111111","score":"92"},
            {"id":"22222222-2222-2222-2222-222222222222","score":"97"}
        ]}"#;
        assert_eq!(
            parse_search_response(body.as_bytes()).as_deref(),
            Some("22222222-2222-2222-2222-222222222222")
        );
    }

    #[test]
    fn search_response_rejects_weak_or_malformed() {
        assert_eq!(parse_search_response(b"not json"), None);
        assert_eq!(parse_search_response(br#"{"releases":[]}"#), None);
        assert_eq!(parse_search_response(br#"{}"#), None);
        // Below the minimum score.
        let weak = format!(r#"{{"releases":[{{"id":"{MBID}","score":60}}]}}"#);
        assert_eq!(parse_search_response(weak.as_bytes()), None);
        // Missing score counts as zero.
        let no_score = format!(r#"{{"releases":[{{"id":"{MBID}"}}]}}"#);
        assert_eq!(parse_search_response(no_score.as_bytes()), None);
        // Invalid id is skipped in favour of a valid one.
        let body = format!(
            r#"{{"releases":[{{"id":"bogus","score":100}},{{"id":"{MBID}","score":91}}]}}"#
        );
        assert_eq!(
            parse_search_response(body.as_bytes()).as_deref(),
            Some(MBID)
        );
    }

    #[test]
    fn rate_limiter_slots() {
        let t0 = Instant::now();
        let sec = Duration::from_secs(1);
        assert_eq!(slot_delay(None, t0, sec), Duration::ZERO);
        assert_eq!(slot_delay(Some(t0), t0, sec), sec);
        assert_eq!(
            slot_delay(Some(t0), t0 + Duration::from_millis(400), sec),
            Duration::from_millis(600)
        );
        assert_eq!(slot_delay(Some(t0), t0 + sec, sec), Duration::ZERO);
        assert_eq!(slot_delay(Some(t0), t0 + sec * 5, sec), Duration::ZERO);
    }

    #[test]
    fn user_agent_has_product_and_contact() {
        let ua = user_agent(None);
        assert!(ua.starts_with("rmpd/"));
        assert!(ua.contains(DEFAULT_CONTACT));
        assert!(user_agent(Some("me@example.org")).ends_with("( me@example.org )"));
    }

    #[test]
    fn mime_resolution() {
        assert_eq!(
            resolve_mime(Some("text/html"), b"\xFF\xD8\xFFdata").as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(
            resolve_mime(Some("image/png"), b"\x89PNG\r\n\x1a\nxx").as_deref(),
            Some("image/png")
        );
        assert_eq!(
            resolve_mime(Some("image/jxl; charset=binary"), b"????").as_deref(),
            Some("image/jxl")
        );
        assert_eq!(resolve_mime(Some("text/html"), b"<html>"), None);
        assert_eq!(resolve_mime(None, b"<html>"), None);
        assert_eq!(resolve_mime(Some("image/"), b"x"), None);
    }

    #[test]
    fn config_defaults_and_validation() {
        let p = CoverArtArchive::from_config(&cfg(&[])).unwrap();
        assert!(!p.lookup);
        assert_eq!(p.size, "500");
        assert_eq!(p.caa_url, DEFAULT_CAA_URL);
        assert_eq!(p.mb_url, DEFAULT_MB_URL);
        assert_eq!(p.name(), "caa");

        let p = CoverArtArchive::from_config(&cfg(&[
            ("lookup", toml::Value::Boolean(true)),
            ("size", toml::Value::Integer(1200)),
            ("caa_url", toml::Value::String("http://localhost:1/".into())),
        ]))
        .unwrap();
        assert!(p.lookup);
        assert_eq!(p.size, "1200");
        assert_eq!(p.caa_url, "http://localhost:1");

        let bad_size = CoverArtArchive::from_config(&cfg(&[("size", "999".into())]));
        assert!(matches!(bad_size, Err(PluginError::Config(_))));
        let bad_url = CoverArtArchive::from_config(&cfg(&[("mb_url", "ftp://x".into())]));
        assert!(matches!(bad_url, Err(PluginError::Config(_))));
        let bad_contact = CoverArtArchive::from_config(&cfg(&[("contact", "a\nb".into())]));
        assert!(matches!(bad_contact, Err(PluginError::Config(_))));
    }

    #[tokio::test]
    async fn no_ids_and_no_lookup_is_a_definitive_miss_without_network() {
        let p = CoverArtArchive::from_config(&cfg(&[])).unwrap();
        let s = song(&[("album", "A"), ("artist", "B")]);
        assert_eq!(p.fetch_outcome(&s).await, ArtworkOutcome::NotFound);
        assert_eq!(p.fetch(&s).await, None);
        // lookup = true but nothing to search with: still no network.
        let p = CoverArtArchive::from_config(&cfg(&[("lookup", true.into())])).unwrap();
        assert_eq!(
            p.fetch_outcome(&song(&[("title", "T")])).await,
            ArtworkOutcome::NotFound
        );
    }
}
