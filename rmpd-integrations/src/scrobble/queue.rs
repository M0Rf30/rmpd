// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Offline listen queue persisted as JSON Lines, plus retry backoff.

use super::tracker::Listen;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// Default cap on queued listens (oldest are dropped first).
pub const DEFAULT_MAX_QUEUE: usize = 10_000;

const BACKOFF_BASE: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);

/// Exponential retry backoff: 30 s, 60 s, 120 s, ... capped at 15 min.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Delay before the next retry; advances the schedule.
    pub fn next_delay(&mut self) -> Duration {
        let factor = 1u32 << self.attempt.min(16);
        self.attempt = self.attempt.saturating_add(1);
        BACKOFF_BASE.saturating_mul(factor).min(BACKOFF_MAX)
    }

    /// Call after a successful submission.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// FIFO of pending listens with optional on-disk persistence.
#[derive(Debug)]
pub struct ListenQueue {
    path: Option<PathBuf>,
    items: VecDeque<Listen>,
    cap: usize,
}

impl ListenQueue {
    /// A queue that is never persisted (tests, or when no state dir exists).
    #[must_use]
    pub fn in_memory(cap: usize) -> Self {
        Self {
            path: None,
            items: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Load (or create) the queue file. Corrupt lines are skipped; if more
    /// than `cap` entries are present the oldest are discarded.
    #[must_use]
    pub fn load(path: PathBuf, cap: usize) -> Self {
        let mut q = Self {
            path: Some(path),
            items: VecDeque::new(),
            cap: cap.max(1),
        };
        let mut dirty = false;
        if let Some(p) = &q.path
            && let Ok(text) = std::fs::read_to_string(p)
        {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                match serde_json::from_str::<Listen>(line) {
                    Ok(l) => q.items.push_back(l),
                    Err(_) => dirty = true,
                }
            }
        }
        while q.items.len() > q.cap {
            q.items.pop_front();
            dirty = true;
        }
        if dirty {
            q.rewrite();
        }
        q
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Append a listen, evicting the oldest entries beyond the cap.
    pub fn push(&mut self, listen: Listen) {
        self.items.push_back(listen);
        let mut evicted = false;
        while self.items.len() > self.cap {
            self.items.pop_front();
            evicted = true;
        }
        if evicted {
            tracing::warn!("scrobble queue full, dropping oldest listens");
            self.rewrite();
        } else if let Some(last) = self.items.back() {
            self.append(last);
        }
    }

    /// Copy of up to `n` oldest listens.
    #[must_use]
    pub fn peek_batch(&self, n: usize) -> Vec<Listen> {
        self.items.iter().take(n).cloned().collect()
    }

    /// Remove the `n` oldest listens (after they were submitted or rejected).
    pub fn drop_front(&mut self, n: usize) {
        for _ in 0..n {
            self.items.pop_front();
        }
        self.rewrite();
    }

    fn append(&self, listen: &Listen) {
        let Some(path) = &self.path else { return };
        let Ok(mut line) = serde_json::to_string(listen) else {
            return;
        };
        line.push('\n');
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = res {
            tracing::warn!("cannot persist scrobble queue: {e}");
        }
    }

    fn rewrite(&self) {
        let Some(path) = &self.path else { return };
        if self.items.is_empty() {
            if let Err(e) = std::fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("cannot clear scrobble queue file: {e}");
            }
            return;
        }
        let mut body = String::new();
        for l in &self.items {
            if let Ok(s) = serde_json::to_string(l) {
                body.push_str(&s);
                body.push('\n');
            }
        }
        let tmp = path.with_extension("jsonl.tmp");
        let res = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = res {
            tracing::warn!("cannot persist scrobble queue: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tracker::TrackMeta;
    use super::*;

    fn listen(n: i64) -> Listen {
        Listen {
            meta: TrackMeta {
                artist: "A".to_owned(),
                title: format!("T{n}"),
                ..TrackMeta::default()
            },
            listened_at: n,
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rmpd-queue-{tag}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let mut b = Backoff::new();
        assert_eq!(b.next_delay(), Duration::from_secs(30));
        assert_eq!(b.next_delay(), Duration::from_secs(60));
        assert_eq!(b.next_delay(), Duration::from_secs(120));
        for _ in 0..40 {
            assert!(b.next_delay() <= BACKOFF_MAX);
        }
        assert_eq!(b.next_delay(), BACKOFF_MAX);
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(30));
    }

    #[test]
    fn cap_drops_oldest() {
        let mut q = ListenQueue::in_memory(3);
        for n in 1..=5 {
            q.push(listen(n));
        }
        assert_eq!(q.len(), 3);
        let b = q.peek_batch(10);
        assert_eq!(
            b.iter().map(|l| l.listened_at).collect::<Vec<_>>(),
            [3, 4, 5]
        );
    }

    #[test]
    fn batch_and_drop_front() {
        let mut q = ListenQueue::in_memory(10);
        for n in 1..=4 {
            q.push(listen(n));
        }
        assert_eq!(q.peek_batch(2).len(), 2);
        q.drop_front(2);
        assert_eq!(q.peek_batch(10)[0].listened_at, 3);
        q.drop_front(10);
        assert!(q.is_empty());
    }

    #[test]
    fn persists_and_reloads_skipping_garbage() {
        let path = temp_path("persist");
        let _ = std::fs::remove_file(&path);
        {
            let mut q = ListenQueue::load(path.clone(), 10);
            q.push(listen(1));
            q.push(listen(2));
            q.push(listen(3));
            q.drop_front(1);
        }
        // Append a corrupt line, as after a crash mid-write.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open");
            f.write_all(b"{not json\n").expect("write");
        }
        let q = ListenQueue::load(path.clone(), 10);
        assert_eq!(
            q.peek_batch(10)
                .iter()
                .map(|l| l.listened_at)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        // Reload with a smaller cap keeps the newest.
        let q = ListenQueue::load(path.clone(), 1);
        assert_eq!(q.peek_batch(10)[0].listened_at, 3);
        let mut q = q;
        q.drop_front(1);
        assert!(!path.exists(), "empty queue removes its file");
    }
}
