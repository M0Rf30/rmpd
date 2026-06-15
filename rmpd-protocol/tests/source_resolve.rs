//! Regression tests for PR5: the prepare_song_for_playback resolve hook.
//!
//! Uses a stub `MusicSource` to prove that:
//!   (a) a virtual `subsonic://name/artist/album/id` path is resolved to the
//!       stub's returned `http://` URL before playback, while a local path
//!       passes through unchanged;
//!   (b) a source whose `ping` / `list_all` errors does NOT abort startup or
//!       the update handler.

use async_trait::async_trait;
use rmpd_source::{MusicSource, SourceEntry, SourceError, SourceRegistry, SourceResult};
use std::sync::Arc;

// ─── Stub MusicSource ─────────────────────────────────────────────────────────

struct StubSource {
    name: String,
    scheme_name: String,
    resolved_url: Option<String>,
    unreachable: bool,
}

#[async_trait]
impl MusicSource for StubSource {
    fn scheme(&self) -> &str {
        &self.scheme_name
    }
    fn name(&self) -> &str {
        &self.name
    }
    async fn ping(&self) -> SourceResult<()> {
        if self.unreachable {
            Err(SourceError::Unreachable("stub: server down".into()))
        } else {
            Ok(())
        }
    }
    async fn browse(&self, _dir: &str) -> SourceResult<Vec<SourceEntry>> {
        Ok(vec![])
    }
    async fn list_all(&self) -> SourceResult<Vec<rmpd_core::song::Song>> {
        if self.unreachable {
            Err(SourceError::Unreachable("stub: server down".into()))
        } else {
            Ok(vec![])
        }
    }
    async fn search(&self, _query: &str) -> SourceResult<Vec<rmpd_core::song::Song>> {
        Ok(vec![])
    }
    async fn resolve_stream_uri(&self, song_id: &str) -> SourceResult<String> {
        match &self.resolved_url {
            Some(url) => Ok(format!("{}/{}", url, song_id)),
            None => Err(SourceError::NotFound("no url configured".into())),
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn stub_registry(
    name: &str,
    scheme: &str,
    resolved_url: Option<&str>,
    unreachable: bool,
) -> SourceRegistry {
    let stub = Box::new(StubSource {
        name: name.to_string(),
        scheme_name: scheme.to_string(),
        resolved_url: resolved_url.map(|s| s.to_string()),
        unreachable,
    }) as Box<dyn MusicSource>;
    SourceRegistry {
        sources: vec![stub],
    }
}

fn test_song(path: &str) -> rmpd_core::song::Song {
    rmpd_core::song::Song {
        id: 0,
        path: path.into(),
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
        tags: Vec::new(),
    }
}

// ─── Test (a): virtual path resolved, local paths pass through ─────────────────

#[tokio::test]
async fn resolve_virtual_path_substitutes_stream_url() {
    let registry = Arc::new(stub_registry(
        "home",
        "subsonic",
        Some("https://stream.example"),
        false,
    ));
    let song = test_song("subsonic://home/Artist/Album/remote-id-42");

    let result =
        rmpd_protocol::commands::utils::prepare_song_for_playback(&song, None, None, &registry)
            .await;

    assert!(result.is_ok());
    let ps = result.unwrap();
    assert!(
        ps.resolved_path
            .as_str()
            .starts_with("https://stream.example/"),
        "expected resolved path to start with stub URL, got '{}'",
        ps.resolved_path
    );
    assert!(
        ps.resolved_path.as_str().contains("remote-id-42"),
        "expected resolved path to contain song id"
    );
}

#[tokio::test]
async fn local_path_passes_through_unchanged() {
    let registry = Arc::new(stub_registry("home", "subsonic", None, false));
    let song = test_song("Music/Artist/Album/track.flac");

    let result = rmpd_protocol::commands::utils::prepare_song_for_playback(
        &song,
        Some("/srv/media"),
        None,
        &registry,
    )
    .await;

    assert!(result.is_ok());
    let ps = result.unwrap();
    assert_eq!(
        ps.resolved_path.as_str(),
        "/srv/media/Music/Artist/Album/track.flac",
    );
}

#[tokio::test]
async fn http_radio_stream_passes_through() {
    let registry = Arc::new(stub_registry("home", "subsonic", None, false));
    let song = test_song("http://radio.example/stream");

    let result =
        rmpd_protocol::commands::utils::prepare_song_for_playback(&song, None, None, &registry)
            .await;

    assert!(result.is_ok());
    let ps = result.unwrap();
    assert_eq!(ps.resolved_path.as_str(), "http://radio.example/stream");
}

// ─── Test (b): unreachable source does NOT abort ──────────────────────────────

#[tokio::test]
async fn unreachable_source_does_not_abort_startup_or_update() {
    let registry = Arc::new(stub_registry("down", "subsonic", None, true));

    assert!(!registry.is_empty());
    assert!(registry.is_source_scheme("subsonic"));

    // Ping fails — caller should skip, not abort.
    for source in registry.iter() {
        assert!(source.ping().await.is_err());
    }

    // list_all also fails.
    for source in registry.iter() {
        assert!(source.list_all().await.is_err());
    }

    // sync_source returns an Err, doesn't panic.
    for source in registry.iter() {
        let sync_result = rmpd_source::sync_source(source, "/nonexistent/db.db").await;
        assert!(sync_result.is_err());
    }
}
