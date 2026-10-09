// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Artwork SPI: cover-art providers consulted by `albumart` / `readpicture`
//! when a song has neither a standalone cover file nor an embedded picture.
//!
//! Concrete providers live in `rmpd-integrations` (which owns the
//! compile-time `ARTWORK_PLUGINS` registry); the protocol layer holds an
//! [`ArtworkResolver`] built from the configured `[[artwork]]` blocks.

use crate::error::PluginError;
use async_trait::async_trait;
use rmpd_core::config::ArtworkConfig;
use rmpd_core::song::Song;
use std::sync::Arc;

/// Result of asking a provider (or a chain of providers) for cover art.
///
/// The distinction between [`NotFound`](Self::NotFound) and
/// [`Unavailable`](Self::Unavailable) drives the negative cache: a definitive
/// miss is remembered for days, a transient failure (network down, rate
/// limited, server error) only briefly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkOutcome {
    /// Image bytes and their MIME type.
    Found(Vec<u8>, String),
    /// The provider answered: it has no art for this song.
    NotFound,
    /// The provider could not answer right now; do not cache for long.
    Unavailable,
}

/// A source of cover art keyed by song tags.
#[async_trait]
pub trait ArtworkProvider: Send + Sync {
    /// Instance name (from the `[[artwork]]` block).
    fn name(&self) -> &str;

    /// Fetch cover art for `song`, using its tags only (never its path).
    /// `None` when nothing is found.
    async fn fetch(&self, song: &Song) -> Option<(Vec<u8>, String)>;

    /// Like [`fetch`](Self::fetch) but able to report a transient failure.
    /// The default maps `fetch` onto `Found` / `NotFound`; providers doing
    /// network I/O override this so failures are not cached as definitive
    /// misses.
    async fn fetch_outcome(&self, song: &Song) -> ArtworkOutcome {
        match self.fetch(song).await {
            Some((data, mime)) => ArtworkOutcome::Found(data, mime),
            None => ArtworkOutcome::NotFound,
        }
    }
}

/// Sync, no-I/O factory: build a provider from its `[[artwork]]` block or fail
/// with [`PluginError::Config`].
pub type ArtworkFactory = fn(&ArtworkConfig) -> Result<Box<dyn ArtworkProvider>, PluginError>;

/// One registry entry for an artwork provider type.
#[derive(Clone, Copy)]
pub struct ArtworkPlugin {
    /// `type = "..."` value (lowercase).
    pub name: &'static str,
    /// Accepted setting keys (for unknown-key diagnostics).
    pub settings: &'static [&'static str],
    pub factory: ArtworkFactory,
}

impl std::fmt::Debug for ArtworkPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtworkPlugin")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Ordered chain of providers: the first one that finds art wins.
#[derive(Clone, Default)]
pub struct ArtworkResolver {
    providers: Vec<Arc<dyn ArtworkProvider>>,
}

impl std::fmt::Debug for ArtworkResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtworkResolver")
            .field(
                "providers",
                &self.providers.iter().map(|p| p.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ArtworkResolver {
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn ArtworkProvider>>) -> Self {
        Self { providers }
    }

    /// `true` when no provider is configured (remote lookup disabled).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Ask each provider in order. `Found` as soon as one finds art;
    /// otherwise `Unavailable` if any provider could not answer, else
    /// `NotFound`.
    pub async fn resolve(&self, song: &Song) -> ArtworkOutcome {
        let mut unavailable = false;
        for provider in &self.providers {
            match provider.fetch_outcome(song).await {
                found @ ArtworkOutcome::Found(..) => return found,
                ArtworkOutcome::Unavailable => unavailable = true,
                ArtworkOutcome::NotFound => {}
            }
        }
        if unavailable {
            ArtworkOutcome::Unavailable
        } else {
            ArtworkOutcome::NotFound
        }
    }
}

/// Album-level cache key for remote artwork, shared by every track of an
/// album: `mbid:<MUSICBRAINZ_ALBUMID>` when tagged, otherwise
/// `album:<albumartist>\u{1f}<album>` (lowercased). `None` when the song has
/// too little metadata to look anything up.
#[must_use]
pub fn artwork_cache_key(song: &Song) -> Option<String> {
    if let Some(mbid) = non_empty(song.tag("musicbrainz_albumid")) {
        return Some(format!("mbid:{}", mbid.to_ascii_lowercase()));
    }
    let album = non_empty(song.tag("album"))?;
    let artist = non_empty(song.tag_with_fallback("albumartist"))?;
    Some(format!(
        "album:{}\u{1f}{}",
        artist.to_lowercase(),
        album.to_lowercase()
    ))
}

fn non_empty(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::intern_tag_key;

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
    fn key_prefers_mbid() {
        let s = song(&[
            ("album", "A"),
            ("artist", "B"),
            ("musicbrainz_albumid", " ABC-123 "),
        ]);
        assert_eq!(artwork_cache_key(&s).as_deref(), Some("mbid:abc-123"));
    }

    #[test]
    fn key_falls_back_to_album_artist() {
        let s = song(&[("album", "Abbey Road"), ("artist", "The Beatles")]);
        assert_eq!(
            artwork_cache_key(&s).as_deref(),
            Some("album:the beatles\u{1f}abbey road")
        );
        let s = song(&[
            ("album", "Abbey Road"),
            ("artist", "x"),
            ("albumartist", "The Beatles"),
        ]);
        assert_eq!(
            artwork_cache_key(&s).as_deref(),
            Some("album:the beatles\u{1f}abbey road")
        );
    }

    #[test]
    fn key_requires_enough_metadata() {
        assert_eq!(artwork_cache_key(&song(&[("title", "T")])), None);
        assert_eq!(artwork_cache_key(&song(&[("album", "A")])), None);
        assert_eq!(
            artwork_cache_key(&song(&[("album", "  "), ("artist", "B")])),
            None
        );
    }

    struct Fixed(&'static str, ArtworkOutcome);

    #[async_trait]
    impl ArtworkProvider for Fixed {
        fn name(&self) -> &str {
            self.0
        }
        async fn fetch(&self, _song: &Song) -> Option<(Vec<u8>, String)> {
            match &self.1 {
                ArtworkOutcome::Found(d, m) => Some((d.clone(), m.clone())),
                _ => None,
            }
        }
        async fn fetch_outcome(&self, _song: &Song) -> ArtworkOutcome {
            self.1.clone()
        }
    }

    fn chain(outcomes: Vec<ArtworkOutcome>) -> ArtworkResolver {
        ArtworkResolver::new(
            outcomes
                .into_iter()
                .map(|o| Arc::new(Fixed("p", o)) as Arc<dyn ArtworkProvider>)
                .collect(),
        )
    }

    #[tokio::test]
    async fn resolver_first_found_wins() {
        let s = song(&[("album", "A"), ("artist", "B")]);
        let r = chain(vec![
            ArtworkOutcome::NotFound,
            ArtworkOutcome::Found(vec![1], "image/png".into()),
            ArtworkOutcome::Found(vec![2], "image/jpeg".into()),
        ]);
        assert_eq!(
            r.resolve(&s).await,
            ArtworkOutcome::Found(vec![1], "image/png".into())
        );
    }

    #[tokio::test]
    async fn resolver_miss_states() {
        let s = song(&[("album", "A"), ("artist", "B")]);
        assert_eq!(chain(vec![]).resolve(&s).await, ArtworkOutcome::NotFound);
        assert!(chain(vec![]).is_empty());
        assert_eq!(
            chain(vec![ArtworkOutcome::NotFound, ArtworkOutcome::NotFound])
                .resolve(&s)
                .await,
            ArtworkOutcome::NotFound
        );
        assert_eq!(
            chain(vec![ArtworkOutcome::Unavailable, ArtworkOutcome::NotFound])
                .resolve(&s)
                .await,
            ArtworkOutcome::Unavailable
        );
    }

    #[tokio::test]
    async fn default_fetch_outcome_maps_fetch() {
        struct Plain;
        #[async_trait]
        impl ArtworkProvider for Plain {
            fn name(&self) -> &str {
                "plain"
            }
            async fn fetch(&self, _song: &Song) -> Option<(Vec<u8>, String)> {
                None
            }
        }
        let s = song(&[]);
        assert_eq!(Plain.fetch_outcome(&s).await, ArtworkOutcome::NotFound);
    }
}
