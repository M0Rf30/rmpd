// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared scrobbling machinery: the [`tracker::ListenTracker`] state machine,
//! the persistent offline [`queue::ListenQueue`], and [`run_scrobbler`], the
//! event loop that wires a [`ScrobbleBackend`] (ListenBrainz, Last.fm, ...)
//! to the player.

pub mod queue;
pub mod tracker;

use async_trait::async_trait;
use queue::{Backoff, DEFAULT_MAX_QUEUE, ListenQueue};
use rmpd_core::config::IntegrationConfig;
use rmpd_core::event::Event;
use rmpd_plugin::PluginError;
use rmpd_plugin::integration::IntegrationContext;
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};
use tracker::{Action, Listen, ListenTracker, TrackMeta, TrackerSettings};

/// Setting keys common to every scrobbler.
pub const COMMON_SETTINGS: &[&str] = &["scrobble_streams", "max_queue"];

/// Timeout for each HTTP request to a scrobbling service.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a submission failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    /// Retry later (network error, rate limit, server error, bad credentials
    /// that the user may still fix). The listens stay queued.
    Transient(String),
    /// The service rejected the payload for good; the listens are dropped.
    Permanent(String),
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Transient(m) => write!(f, "temporary failure: {m}"),
            SubmitError::Permanent(m) => write!(f, "rejected: {m}"),
        }
    }
}

impl std::error::Error for SubmitError {}

/// A scrobbling service. Error messages MUST NOT contain credentials.
#[async_trait]
pub trait ScrobbleBackend: Send + Sync + 'static {
    /// Largest number of listens accepted in one `submit` call.
    fn max_batch(&self) -> usize;
    /// Best-effort "now playing" update (never retried).
    async fn now_playing(&self, meta: &TrackMeta) -> Result<(), SubmitError>;
    /// Submit listens, oldest first.
    async fn submit(&self, listens: &[Listen]) -> Result<(), SubmitError>;
}

/// Options shared by all scrobblers.
#[derive(Debug, Clone, Copy)]
pub struct ScrobbleSettings {
    pub scrobble_streams: bool,
    pub max_queue: usize,
}

impl ScrobbleSettings {
    /// Parse `scrobble_streams` (default false) and `max_queue`
    /// (default 10000) from an `[[integration]]` block.
    pub fn from_config(cfg: &IntegrationConfig) -> Result<Self, PluginError> {
        let max_queue = match cfg.setting_str("max_queue") {
            None => DEFAULT_MAX_QUEUE,
            Some(s) => s
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| PluginError::Config(format!("invalid max_queue: {s}")))?,
        };
        Ok(Self {
            scrobble_streams: setting_bool(cfg, "scrobble_streams", false)?,
            max_queue,
        })
    }
}

/// Read a boolean setting (accepts TOML booleans and `true/false/yes/no/on/off/1/0`).
pub fn setting_bool(
    cfg: &IntegrationConfig,
    key: &str,
    default: bool,
) -> Result<bool, PluginError> {
    match cfg.setting_str(key) {
        None => Ok(default),
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(true),
            "false" | "no" | "off" | "0" => Ok(false),
            _ => Err(PluginError::Config(format!("{key} must be a boolean"))),
        },
    }
}

/// Build the shared HTTP client used by the scrobblers.
pub fn http_client() -> Result<reqwest::Client, PluginError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("rmpd/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| PluginError::Runtime(format!("cannot build HTTP client: {}", e.without_url())))
}

/// Map a `reqwest` transport error to a transient [`SubmitError`] (URL
/// stripped so nothing sensitive can leak into logs).
pub fn transport_error(e: reqwest::Error) -> SubmitError {
    SubmitError::Transient(e.without_url().to_string())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Queue an action for the worker without blocking the event loop.
fn dispatch(tx: &mpsc::Sender<Action>, name: &str, actions: Vec<Action>) {
    for a in actions {
        if let Err(e) = tx.try_send(a) {
            tracing::warn!(%name, "scrobbler busy, dropping update: {e}");
        }
    }
}

/// Drive `backend` from the player until shutdown: track listens, send
/// now-playing, and submit completed listens through the offline queue.
pub async fn run_scrobbler<B: ScrobbleBackend>(
    name: String,
    backend: B,
    settings: ScrobbleSettings,
    ctx: IntegrationContext,
) -> Result<(), PluginError> {
    let IntegrationContext {
        mut events,
        player,
        state_dir,
        mut shutdown,
    } = ctx;
    let queue = ListenQueue::load(state_dir.join("queue.jsonl"), settings.max_queue);
    if !queue.is_empty() {
        tracing::info!(%name, pending = queue.len(), "resuming scrobble queue");
    }
    let (tx, rx) = mpsc::channel::<Action>(256);

    let worker = worker(name.clone(), backend, queue, rx);

    let front = async move {
        let mut tracker = ListenTracker::new(TrackerSettings {
            scrobble_streams: settings.scrobble_streams,
        });
        let snap = player.status().await;
        dispatch(
            &tx,
            &name,
            tracker.resync(snap.state, snap.song.as_deref(), Instant::now(), unix_now()),
        );
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let actions = tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => tracker.tick(Instant::now()),
                ev = events.recv() => match ev {
                    Ok(ev) => handle_event(&mut tracker, ev),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(%name, skipped = n, "event stream lagged, resyncing");
                        let snap = player.status().await;
                        tracker.resync(snap.state, snap.song.as_deref(), Instant::now(), unix_now())
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            dispatch(&tx, &name, actions);
        }
        // Dropping `tx` lets the worker drain and finish.
    };

    tokio::join!(front, worker);
    Ok(())
}

fn handle_event(tracker: &mut ListenTracker, ev: Event) -> Vec<Action> {
    let now = Instant::now();
    match ev {
        Event::SongChanged(song) => tracker.song_changed(song.as_ref(), now, unix_now()),
        Event::PlayerStateChanged(state) => tracker.state_changed(state, now),
        Event::PositionChanged(pos) => tracker.position(pos, now),
        // `AdvancedToNext` is followed by `SongChanged` for the new song.
        Event::SongFinished | Event::AdvancedToNext => {
            tracker.track_finished();
            Vec::new()
        }
        Event::StreamTitleChanged(title) => tracker.stream_title(title.as_deref(), now, unix_now()),
        _ => Vec::new(),
    }
}

/// Outcome of one flush attempt.
async fn flush<B: ScrobbleBackend>(
    name: &str,
    backend: &B,
    queue: &mut ListenQueue,
    backoff: &mut Backoff,
) -> Option<Duration> {
    let mut batch_size = backend.max_batch().max(1);
    while !queue.is_empty() {
        let batch = queue.peek_batch(batch_size);
        match backend.submit(&batch).await {
            Ok(()) => {
                tracing::debug!(%name, count = batch.len(), "scrobbled");
                queue.drop_front(batch.len());
                backoff.reset();
            }
            Err(SubmitError::Transient(msg)) => {
                let delay = backoff.next_delay();
                tracing::warn!(
                    %name,
                    pending = queue.len(),
                    retry_in = ?delay,
                    "scrobble submission failed: {msg}"
                );
                return Some(delay);
            }
            Err(SubmitError::Permanent(msg)) => {
                tracing::warn!(%name, count = batch.len(), "scrobble rejected, dropping: {msg}");
                if batch.len() > 1 {
                    // Isolate the offending listen.
                    batch_size = 1;
                } else {
                    queue.drop_front(1);
                }
            }
        }
    }
    None
}

async fn worker<B: ScrobbleBackend>(
    name: String,
    backend: B,
    mut queue: ListenQueue,
    mut rx: mpsc::Receiver<Action>,
) {
    let mut backoff = Backoff::new();
    let mut retry_at: Option<tokio::time::Instant> =
        (!queue.is_empty()).then(tokio::time::Instant::now);
    loop {
        let deadline = retry_at;
        let wait = async move {
            match deadline {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            msg = rx.recv() => match msg {
                None => {
                    // Shutting down: one last bounded attempt to drain.
                    if !queue.is_empty() {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            flush(&name, &backend, &mut queue, &mut backoff),
                        )
                        .await;
                    }
                    return;
                }
                Some(Action::NowPlaying(meta)) => {
                    // Skip while backing off: the service is unreachable.
                    if retry_at.is_none()
                        && let Err(e) = backend.now_playing(&meta).await
                    {
                        tracing::debug!(%name, "now-playing update failed: {e}");
                    }
                }
                Some(Action::Listen(listen)) => {
                    tracing::debug!(%name, artist = %listen.meta.artist, title = %listen.meta.title, "listen due");
                    queue.push(listen);
                    if retry_at.is_none() {
                        retry_at = Some(tokio::time::Instant::now());
                    }
                }
            },
            () = wait => {
                retry_at = flush(&name, &backend, &mut queue, &mut backoff)
                    .await
                    .map(|d| tokio::time::Instant::now() + d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        fail_first: AtomicUsize,
        reject_title: Option<&'static str>,
        sent: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ScrobbleBackend for Fake {
        fn max_batch(&self) -> usize {
            3
        }
        async fn now_playing(&self, _meta: &TrackMeta) -> Result<(), SubmitError> {
            Ok(())
        }
        async fn submit(&self, listens: &[Listen]) -> Result<(), SubmitError> {
            if self.fail_first.load(Ordering::SeqCst) > 0 {
                self.fail_first.fetch_sub(1, Ordering::SeqCst);
                return Err(SubmitError::Transient("down".to_owned()));
            }
            if let Some(bad) = self.reject_title
                && listens.iter().any(|l| l.meta.title == bad)
            {
                return Err(SubmitError::Permanent("bad".to_owned()));
            }
            let mut sent = self.sent.lock().expect("lock");
            sent.extend(listens.iter().map(|l| l.meta.title.clone()));
            Ok(())
        }
    }

    fn listen(t: &str) -> Listen {
        Listen {
            meta: TrackMeta {
                artist: "A".to_owned(),
                title: t.to_owned(),
                ..TrackMeta::default()
            },
            listened_at: 1,
        }
    }

    #[tokio::test]
    async fn flush_retries_then_drains_in_batches() {
        let fake = Fake {
            fail_first: AtomicUsize::new(1),
            reject_title: None,
            sent: Mutex::new(Vec::new()),
        };
        let mut q = ListenQueue::in_memory(10);
        for t in ["1", "2", "3", "4", "5"] {
            q.push(listen(t));
        }
        let mut b = Backoff::new();
        let delay = flush("t", &fake, &mut q, &mut b).await;
        assert_eq!(delay, Some(Duration::from_secs(30)));
        assert_eq!(q.len(), 5, "nothing lost on transient failure");
        assert_eq!(flush("t", &fake, &mut q, &mut b).await, None);
        assert!(q.is_empty());
        assert_eq!(*fake.sent.lock().expect("lock"), ["1", "2", "3", "4", "5"]);
    }

    #[tokio::test]
    async fn permanent_failure_isolates_bad_listen() {
        let fake = Fake {
            fail_first: AtomicUsize::new(0),
            reject_title: Some("2"),
            sent: Mutex::new(Vec::new()),
        };
        let mut q = ListenQueue::in_memory(10);
        for t in ["1", "2", "3"] {
            q.push(listen(t));
        }
        let mut b = Backoff::new();
        assert_eq!(flush("t", &fake, &mut q, &mut b).await, None);
        assert!(q.is_empty());
        assert_eq!(*fake.sent.lock().expect("lock"), ["1", "3"]);
    }
}
