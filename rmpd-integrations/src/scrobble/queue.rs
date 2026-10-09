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

#[derive(serde::Deserialize)]
struct DropMarker {
    drop: usize,
}

/// Minimum number of dead listen lines before the file is compacted.
const COMPACT_MIN_DEAD: usize = 256;

/// FIFO of pending listens with optional on-disk persistence.
///
/// The file is an append-only log: one JSON listen per line plus
/// `{"drop":n}` markers that remove the `n` oldest entries.
#[derive(Debug)]
pub struct ListenQueue {
    path: Option<PathBuf>,
    items: VecDeque<Listen>,
    cap: usize,
    /// Listen lines currently in the file (live + dead).
    file_listens: usize,
}

impl ListenQueue {
    /// A queue that is never persisted (tests, or when no state dir exists).
    #[must_use]
    pub fn in_memory(cap: usize) -> Self {
        Self {
            path: None,
            items: VecDeque::new(),
            cap: cap.max(1),
            file_listens: 0,
        }
    }

    /// Load (or create) the queue file. Corrupt lines are skipped; if more
    /// than `cap` entries are present the oldest are discarded. Drop markers
    /// (`{"drop":n}`) written by [`Self::drop_front`] are replayed.
    #[must_use]
    pub fn load(path: PathBuf, cap: usize) -> Self {
        let mut q = Self {
            path: Some(path),
            items: VecDeque::new(),
            cap: cap.max(1),
            file_listens: 0,
        };
        let mut dirty = false;
        if let Some(p) = &q.path
            && let Ok(text) = std::fs::read_to_string(p)
        {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(l) = serde_json::from_str::<Listen>(line) {
                    q.items.push_back(l);
                    q.file_listens += 1;
                } else if let Ok(m) = serde_json::from_str::<DropMarker>(line) {
                    let n = m.drop.min(q.items.len());
                    q.items.drain(..n);
                } else {
                    dirty = true;
                }
            }
        }
        while q.items.len() > q.cap {
            q.items.pop_front();
            dirty = true;
        }
        if dirty {
            q.rewrite();
        } else {
            q.maybe_compact();
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
        let mut evicted = 0;
        while self.items.len() > self.cap {
            self.items.pop_front();
            evicted += 1;
        }
        if let Some(last) = self.items.back().cloned() {
            self.append(&last);
        }
        if evicted > 0 {
            tracing::warn!("scrobble queue full, dropping oldest listens");
            self.append_drop(evicted);
            self.maybe_compact();
        }
    }

    /// Copy of up to `n` oldest listens.
    #[must_use]
    pub fn peek_batch(&self, n: usize) -> Vec<Listen> {
        self.items.iter().take(n).cloned().collect()
    }

    /// Remove the `n` oldest listens (after they were submitted or rejected).
    ///
    /// Persisted as a single appended drop marker; the file is compacted only
    /// once dead entries dominate, so draining a long queue is amortised O(n).
    pub fn drop_front(&mut self, n: usize) {
        let n = n.min(self.items.len());
        if n == 0 {
            return;
        }
        self.items.drain(..n);
        if self.items.is_empty() {
            self.rewrite();
        } else {
            self.append_drop(n);
            self.maybe_compact();
        }
    }

    fn append_drop(&mut self, n: usize) {
        self.append_raw(&format!("{{\"drop\":{n}}}\n"));
    }

    fn append(&mut self, listen: &Listen) {
        let Ok(mut line) = serde_json::to_string(listen) else {
            return;
        };
        line.push('\n');
        self.append_raw(&line);
        self.file_listens += 1;
    }

    fn append_raw(&self, text: &str) {
        let Some(path) = &self.path else { return };
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(text.as_bytes()));
        if let Err(e) = res {
            tracing::warn!("cannot persist scrobble queue: {e}");
        }
    }

    /// Rewrite the file when more than half of its listen lines are dead.
    fn maybe_compact(&mut self) {
        let dead = self.file_listens.saturating_sub(self.items.len());
        if dead > COMPACT_MIN_DEAD && dead > self.items.len() {
            self.rewrite();
        }
    }

    fn rewrite(&mut self) {
        self.file_listens = self.items.len();
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

    #[test]
    fn drops_are_appended_not_rewritten_and_replayed() {
        let path = temp_path("markers");
        let _ = std::fs::remove_file(&path);
        let mut q = ListenQueue::load(path.clone(), 4);
        for n in 1..=6 {
            q.push(listen(n)); // 5 and 6 evict 1 and 2
        }
        q.drop_front(1);
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text.matches("\"drop\"").count(), 3, "markers appended");
        assert_eq!(text.lines().count(), 9, "no rewrite happened");
        let q = ListenQueue::load(path.clone(), 4);
        assert_eq!(
            q.peek_batch(10)
                .iter()
                .map(|l| l.listened_at)
                .collect::<Vec<_>>(),
            [4, 5, 6]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn compacts_once_dead_entries_dominate() {
        let path = temp_path("compact");
        let _ = std::fs::remove_file(&path);
        let total = i64::try_from(COMPACT_MIN_DEAD).expect("fits") * 2 + 10;
        let mut q = ListenQueue::load(path.clone(), 100_000);
        for n in 0..total {
            q.push(listen(n));
        }
        for _ in 0..(total - 5) {
            q.drop_front(1);
        }
        let lines = std::fs::read_to_string(&path)
            .expect("read")
            .lines()
            .count();
        assert!(lines < usize::try_from(total).expect("fits"), "compacted");
        let q = ListenQueue::load(path.clone(), 100_000);
        assert_eq!(q.len(), 5);
        assert_eq!(q.peek_batch(1)[0].listened_at, total - 5);
        let _ = std::fs::remove_file(&path);
    }
}
