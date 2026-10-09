// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded play history: the songs that recently started playing.
//!
//! [`PlayHistory`] is the plain ring buffer; [`HistoryLog`] is the cheaply
//! clonable, thread-safe handle shared between the recorder, the state file and
//! the API surface.

use crate::song::Song;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of remembered songs (`general.history_length`).
pub const DEFAULT_HISTORY_LENGTH: usize = 1000;

/// One played song.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unix time in milliseconds at which the song started playing.
    pub timestamp_ms: u64,
    /// Song URI (library path or stream URL), as in the queue.
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl HistoryEntry {
    /// Build an entry for `song` started at `timestamp_ms`.
    #[must_use]
    pub fn from_song(song: &Song, timestamp_ms: u64) -> Self {
        Self {
            timestamp_ms,
            uri: song.path.as_str().to_owned(),
            title: non_empty(song.tag("title")),
            artist: non_empty(song.tag("artist").or_else(|| song.tag("albumartist"))),
            album: non_empty(song.tag("album")),
        }
    }

    /// Display name: the title when known, otherwise the last path segment.
    #[must_use]
    pub fn name(&self) -> String {
        match &self.title {
            Some(t) => t.clone(),
            None => self
                .uri
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&self.uri)
                .to_owned(),
        }
    }
}

/// Escape `\\`, tab and line breaks so a field fits in one tab-separated line.
fn escape_field(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn unescape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') | None => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

impl HistoryEntry {
    /// Serialize as `timestamp_ms<TAB>uri<TAB>title<TAB>artist<TAB>album`
    /// (missing tags are empty fields), the value of a state-file
    /// `history:` line. Never contains a line break.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut out = self.timestamp_ms.to_string();
        out.push('\t');
        escape_field(&self.uri, &mut out);
        for tag in [&self.title, &self.artist, &self.album] {
            out.push('\t');
            if let Some(t) = tag {
                escape_field(t, &mut out);
            }
        }
        out
    }

    /// Inverse of [`to_line`](Self::to_line). Tolerates trailing empty fields
    /// being trimmed away; returns `None` for a malformed line.
    #[must_use]
    pub fn from_line(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let timestamp_ms = fields.next()?.trim().parse().ok()?;
        let uri = unescape_field(fields.next()?);
        if uri.is_empty() {
            return None;
        }
        let mut tag = || fields.next().map(unescape_field).filter(|s| !s.is_empty());
        let title = tag();
        let artist = tag();
        let album = tag();
        Some(Self {
            timestamp_ms,
            uri,
            title,
            artist,
            album,
        })
    }
}

/// Current Unix time in milliseconds (0 if the clock is before the epoch).
#[must_use]
pub fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Fixed-capacity ring buffer of [`HistoryEntry`], oldest first internally.
/// A capacity of `0` disables recording.
#[derive(Debug, Clone)]
pub struct PlayHistory {
    entries: VecDeque<HistoryEntry>,
    capacity: usize,
}

impl PlayHistory {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of entries currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Change the capacity, dropping the oldest entries that no longer fit.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.trim();
    }

    /// Append `entry` as the newest, evicting the oldest when full. No-op when
    /// the history is disabled (capacity 0).
    pub fn push(&mut self, entry: HistoryEntry) {
        if self.capacity == 0 {
            return;
        }
        self.entries.push_back(entry);
        self.trim();
    }

    fn trim(&mut self) {
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Entries newest first.
    pub fn iter_newest_first(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter().rev()
    }

    /// Entries oldest first (the order to persist and to `restore` in).
    pub fn iter_oldest_first(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    /// Replace the content with `entries` (oldest first), keeping only the
    /// newest `capacity` of them.
    pub fn restore(&mut self, entries: impl IntoIterator<Item = HistoryEntry>) {
        self.entries.clear();
        for e in entries {
            self.push(e);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for PlayHistory {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_LENGTH)
    }
}

/// Shared, thread-safe [`PlayHistory`].
#[derive(Debug, Clone, Default)]
pub struct HistoryLog(Arc<Mutex<PlayHistory>>);

impl HistoryLog {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(PlayHistory::new(capacity))))
    }

    fn lock(&self) -> MutexGuard<'_, PlayHistory> {
        // The history holds plain data; a panic elsewhere cannot leave it
        // inconsistent, so a poisoned lock is safe to keep using.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn push(&self, entry: HistoryEntry) {
        self.lock().push(entry);
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.lock().set_capacity(capacity);
    }

    pub fn restore(&self, entries: impl IntoIterator<Item = HistoryEntry>) {
        self.lock().restore(entries);
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.lock().capacity()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Snapshot, newest first.
    #[must_use]
    pub fn newest_first(&self) -> Vec<HistoryEntry> {
        self.lock().iter_newest_first().cloned().collect()
    }

    /// Snapshot, oldest first (persistence order).
    #[must_use]
    pub fn oldest_first(&self) -> Vec<HistoryEntry> {
        self.lock().iter_oldest_first().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64) -> HistoryEntry {
        HistoryEntry {
            timestamp_ms: n,
            uri: format!("dir/song{n}.flac"),
            title: Some(format!("Song {n}")),
            artist: None,
            album: None,
        }
    }

    #[test]
    fn newest_first_and_bounded() {
        let mut h = PlayHistory::new(3);
        for n in 1..=5 {
            h.push(entry(n));
        }
        assert_eq!(h.len(), 3);
        let ts: Vec<u64> = h.iter_newest_first().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, [5, 4, 3]);
        let ts: Vec<u64> = h.iter_oldest_first().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, [3, 4, 5]);
    }

    #[test]
    fn zero_capacity_disables_recording() {
        let mut h = PlayHistory::new(0);
        h.push(entry(1));
        assert!(h.is_empty());
    }

    #[test]
    fn shrinking_capacity_drops_oldest() {
        let mut h = PlayHistory::new(5);
        for n in 1..=5 {
            h.push(entry(n));
        }
        h.set_capacity(2);
        let ts: Vec<u64> = h.iter_newest_first().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, [5, 4]);
        h.set_capacity(0);
        assert!(h.is_empty());
    }

    #[test]
    fn restore_keeps_newest_within_capacity() {
        let mut h = PlayHistory::new(2);
        h.push(entry(99));
        h.restore((1..=4).map(entry));
        let ts: Vec<u64> = h.iter_oldest_first().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, [3, 4]);
    }

    #[test]
    fn name_prefers_title_then_last_segment() {
        let mut e = entry(1);
        assert_eq!(e.name(), "Song 1");
        e.title = None;
        assert_eq!(e.name(), "song1.flac");
        e.uri = "http://radio.example/stream".into();
        assert_eq!(e.name(), "stream");
        e.uri = "plain.mp3".into();
        assert_eq!(e.name(), "plain.mp3");
    }

    fn song(path: &str, tags: &[(&'static str, &str)]) -> Song {
        Song {
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
            range: None,
            tags: tags
                .iter()
                .map(|(k, v)| (std::borrow::Cow::Borrowed(*k), (*v).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn from_song_extracts_tags() {
        let s = song(
            "a/b.flac",
            &[
                ("title", " T "),
                ("artist", "A"),
                ("album", ""),
                ("track", "1"),
            ],
        );
        let e = HistoryEntry::from_song(&s, 42);
        assert_eq!(e.timestamp_ms, 42);
        assert_eq!(e.uri, "a/b.flac");
        assert_eq!(e.title.as_deref(), Some("T"));
        assert_eq!(e.artist.as_deref(), Some("A"));
        assert_eq!(e.album, None, "empty tags are dropped");
    }

    #[test]
    fn from_song_falls_back_to_albumartist() {
        let s = song("x.mp3", &[("albumartist", "AA")]);
        assert_eq!(HistoryEntry::from_song(&s, 0).artist.as_deref(), Some("AA"));
    }

    #[test]
    fn line_roundtrip_preserves_awkward_text() {
        let e = HistoryEntry {
            timestamp_ms: 1_700_000_000_123,
            uri: "dir/we\\ird\tname\n.flac".into(),
            title: Some("Tab\there \\ back\\slash".into()),
            artist: None,
            album: Some("Alb".into()),
        };
        let line = e.to_line();
        assert!(!line.contains('\n'), "{line:?}");
        assert_eq!(HistoryEntry::from_line(&line), Some(e));
    }

    #[test]
    fn from_line_tolerates_trimmed_trailing_fields_and_rejects_garbage() {
        let e = HistoryEntry::from_line("5\tsong.mp3").unwrap();
        assert_eq!(e.uri, "song.mp3");
        assert_eq!(e.title, None);
        assert_eq!(HistoryEntry::from_line("notanumber\tx"), None);
        assert_eq!(HistoryEntry::from_line("5"), None);
        assert_eq!(HistoryEntry::from_line("5\t"), None);
    }

    #[test]
    fn shared_log_is_shared_between_clones() {
        let a = HistoryLog::new(2);
        let b = a.clone();
        a.push(entry(1));
        b.push(entry(2));
        b.push(entry(3));
        let ts: Vec<u64> = a.newest_first().iter().map(|e| e.timestamp_ms).collect();
        assert_eq!(ts, [3, 2]);
        assert_eq!(a.capacity(), 2);
    }
}
