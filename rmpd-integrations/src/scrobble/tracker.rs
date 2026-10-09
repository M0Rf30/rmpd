// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure listen-tracking state machine shared by all scrobblers.
//!
//! The tracker is fed player events (already decoded into method calls) plus a
//! monotonic `now: Instant` and a wall-clock `unix` timestamp, and returns
//! [`Action`]s: a "now playing" notification when a track starts, and a
//! [`Listen`] once the track was *actually played* for long enough.
//!
//! Rules (the Last.fm / ListenBrainz convention):
//! * the track must be at least 30 s long (when the length is known);
//! * a listen is due after `min(duration / 2, 240 s)` of played time
//!   (`240 s` when the duration is unknown);
//! * paused time and seeks never count towards played time;
//! * radio streams are optional (`scrobble_streams`); each ICY title
//!   (`Artist - Title`) is a track of its own, due after 60 s.

use rmpd_core::song::Song;
use rmpd_core::state::PlayerState;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Tracks shorter than this are never scrobbled.
pub const MIN_TRACK_LENGTH: Duration = Duration::from_secs(30);
/// Upper bound of the played time required for a listen.
pub const MAX_LISTEN_THRESHOLD: Duration = Duration::from_secs(240);
/// Played time required for an ICY stream title (no duration available).
pub const STREAM_LISTEN_THRESHOLD: Duration = Duration::from_secs(60);
/// A forward position jump larger than the elapsed wall time by more than
/// this is treated as a seek (and not credited).
const SEEK_SLACK: Duration = Duration::from_millis(1500);
/// A repeated "song changed" notification for the same song within this
/// window is a duplicate (e.g. `AdvancedToNext` followed by `SongChanged`).
const DUPLICATE_WINDOW: Duration = Duration::from_secs(2);

/// Metadata submitted to a scrobbling service.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackMeta {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<String>,
    pub duration_secs: Option<u64>,
    /// MusicBrainz recording ID (`MUSICBRAINZ_TRACKID`).
    pub recording_mbid: Option<String>,
    /// MusicBrainz release ID (`MUSICBRAINZ_ALBUMID`).
    pub release_mbid: Option<String>,
    /// MusicBrainz artist IDs (`MUSICBRAINZ_ARTISTID`).
    pub artist_mbids: Vec<String>,
    /// MusicBrainz release group ID.
    pub release_group_mbid: Option<String>,
    /// MusicBrainz release-track ID (`MUSICBRAINZ_RELEASETRACKID`).
    pub track_mbid: Option<String>,
}

/// A completed listen; `listened_at` is the unix time playback started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listen {
    pub meta: TrackMeta,
    pub listened_at: i64,
}

/// What the tracker wants the scrobbler to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    NowPlaying(TrackMeta),
    Listen(Listen),
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// True for a canonical 36-character hyphenated UUID string.
fn valid_mbid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn mbid(song: &Song, tag: &str) -> Option<String> {
    song.tag(tag)
        .map(str::trim)
        .filter(|s| valid_mbid(s))
        .map(str::to_ascii_lowercase)
}

impl TrackMeta {
    /// Build metadata from a library song. Returns `None` when the song has no
    /// usable artist and title tags (such songs are never scrobbled).
    #[must_use]
    pub fn from_song(song: &Song) -> Option<Self> {
        let artist = non_empty(song.tag("artist").or_else(|| song.tag("albumartist")))?;
        let title = non_empty(song.tag("title"))?;
        let track_number = non_empty(song.tag("track").map(|t| t.split('/').next().unwrap_or(t)));
        let artist_mbids = song
            .tag_values("musicbrainz_artistid")
            .map(str::trim)
            .filter(|s| valid_mbid(s))
            .map(str::to_ascii_lowercase)
            .collect();
        Some(Self {
            artist,
            title,
            album: non_empty(song.tag("album")),
            album_artist: non_empty(song.tag("albumartist")),
            track_number,
            duration_secs: song.duration.map(|d| d.as_secs()),
            recording_mbid: mbid(song, "musicbrainz_trackid"),
            release_mbid: mbid(song, "musicbrainz_albumid"),
            artist_mbids,
            release_group_mbid: mbid(song, "musicbrainz_releasegroupid"),
            track_mbid: mbid(song, "musicbrainz_releasetrackid"),
        })
    }
}

/// Split an ICY stream title on `" - "` into `(artist, title)`.
#[must_use]
pub fn parse_icy_title(raw: &str) -> Option<(String, String)> {
    let (artist, title) = raw.split_once(" - ")?;
    let (artist, title) = (artist.trim(), title.trim());
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    Some((artist.to_owned(), title.to_owned()))
}

/// True when `path` refers to a remote stream rather than a library file.
#[must_use]
pub fn is_stream_path(path: &str) -> bool {
    path.contains("://") && !path.starts_with("file://")
}

/// Tracker options.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackerSettings {
    /// Also scrobble radio streams (using ICY titles).
    pub scrobble_streams: bool,
}

#[derive(Debug)]
struct Current {
    meta: TrackMeta,
    started_unix: i64,
    played: Duration,
    now_playing_sent: bool,
    scrobbled: bool,
    is_stream: bool,
}

impl Current {
    fn eligible(&self) -> bool {
        self.is_stream
            || self
                .meta
                .duration_secs
                .is_none_or(|s| Duration::from_secs(s) >= MIN_TRACK_LENGTH)
    }

    fn threshold(&self) -> Duration {
        if self.is_stream {
            return STREAM_LISTEN_THRESHOLD;
        }
        match self.meta.duration_secs {
            Some(s) => (Duration::from_secs(s) / 2).min(MAX_LISTEN_THRESHOLD),
            None => MAX_LISTEN_THRESHOLD,
        }
    }
}

/// Listen tracker. See the module docs for the rules.
#[derive(Debug)]
pub struct ListenTracker {
    settings: TrackerSettings,
    state: PlayerState,
    /// Identity of the loaded song: `(id, path)`.
    song_key: Option<(u64, String)>,
    song_started: Option<Instant>,
    /// The loaded song is a stream whose ICY titles we follow.
    stream_active: bool,
    current: Option<Current>,
    /// Last position report and the instant it was received.
    last_pos: Option<(Duration, Instant)>,
    /// Wall-clock accounting for stream tracks.
    last_tick: Option<Instant>,
}

impl ListenTracker {
    #[must_use]
    pub fn new(settings: TrackerSettings) -> Self {
        Self {
            settings,
            state: PlayerState::Stop,
            song_key: None,
            song_started: None,
            stream_active: false,
            current: None,
            last_pos: None,
            last_tick: None,
        }
    }

    /// Identity of the song the tracker currently knows about.
    #[must_use]
    pub fn song_key(&self) -> Option<&(u64, String)> {
        self.song_key.as_ref()
    }

    /// Played time accumulated for the current track.
    #[must_use]
    pub fn played(&self) -> Duration {
        self.current.as_ref().map_or(Duration::ZERO, |c| c.played)
    }

    fn begin(&mut self, meta: TrackMeta, is_stream: bool, now: Instant, unix: i64) -> Vec<Action> {
        let mut cur = Current {
            meta,
            started_unix: unix,
            played: Duration::ZERO,
            now_playing_sent: false,
            scrobbled: false,
            is_stream,
        };
        self.last_tick = Some(now);
        let mut out = Vec::new();
        if self.state == PlayerState::Play && cur.eligible() {
            cur.now_playing_sent = true;
            out.push(Action::NowPlaying(cur.meta.clone()));
        }
        self.current = Some(cur);
        out
    }

    /// The loaded song changed (`None`: nothing loaded).
    pub fn song_changed(&mut self, song: Option<&Song>, now: Instant, unix: i64) -> Vec<Action> {
        let Some(song) = song else {
            self.song_key = None;
            self.song_started = None;
            self.stream_active = false;
            self.current = None;
            return Vec::new();
        };
        let key = (song.id, song.path.as_str().to_owned());
        if self.song_key.as_ref() == Some(&key)
            && self
                .song_started
                .is_some_and(|t| now.saturating_duration_since(t) < DUPLICATE_WINDOW)
        {
            return Vec::new();
        }
        self.song_key = Some(key);
        self.song_started = Some(now);
        self.last_pos = Some((Duration::ZERO, now));
        self.current = None;
        self.stream_active = false;

        if is_stream_path(song.path.as_str()) {
            self.stream_active = self.settings.scrobble_streams;
            return Vec::new();
        }
        match TrackMeta::from_song(song) {
            Some(meta) => self.begin(meta, false, now, unix),
            None => Vec::new(),
        }
    }

    /// The current song ended (natural end, or advanced to the next one).
    pub fn track_finished(&mut self) {
        self.current = None;
        self.stream_active = false;
        self.last_pos = None;
    }

    /// Player state changed.
    pub fn state_changed(&mut self, state: PlayerState, now: Instant) -> Vec<Action> {
        // Credit stream time up to this instant using the *old* state.
        let mut out = self.tick(now);
        self.state = state;
        match state {
            PlayerState::Play => {
                self.last_tick = Some(now);
                if let Some((pos, _)) = self.last_pos {
                    self.last_pos = Some((pos, now));
                }
                if let Some(cur) = self.current.as_mut()
                    && !cur.now_playing_sent
                    && cur.eligible()
                {
                    cur.now_playing_sent = true;
                    out.push(Action::NowPlaying(cur.meta.clone()));
                }
            }
            PlayerState::Pause => {}
            PlayerState::Stop => {
                self.current = None;
                self.stream_active = false;
                self.last_pos = None;
            }
        }
        out
    }

    /// Playback position report. Only ever credits forward progress that is
    /// consistent with the wall clock; backwards jumps and big forward jumps
    /// are seeks.
    pub fn position(&mut self, pos: Duration, now: Instant) -> Vec<Action> {
        let prev = self.last_pos.replace((pos, now));
        if self.state != PlayerState::Play {
            return Vec::new();
        }
        let Some(cur) = self.current.as_mut() else {
            return Vec::new();
        };
        if cur.is_stream {
            return Vec::new(); // streams are credited by `tick`
        }
        let Some((prev_pos, prev_at)) = prev else {
            return Vec::new();
        };
        if pos < prev_pos {
            return Vec::new(); // seek backwards / restart
        }
        let delta = pos - prev_pos;
        let wall = now.saturating_duration_since(prev_at);
        if delta <= wall + SEEK_SLACK {
            cur.played += delta;
        }
        self.check_listen()
    }

    /// Periodic wall-clock tick; credits played time for stream tracks.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let prev = self.last_tick.replace(now);
        if self.state != PlayerState::Play {
            return Vec::new();
        }
        let (Some(prev), Some(cur)) = (prev, self.current.as_mut()) else {
            return Vec::new();
        };
        if !cur.is_stream {
            return Vec::new();
        }
        cur.played += now.saturating_duration_since(prev);
        self.check_listen()
    }

    /// ICY stream title changed. A no-op unless `scrobble_streams` is on and
    /// the loaded song is a stream.
    pub fn stream_title(&mut self, title: Option<&str>, now: Instant, unix: i64) -> Vec<Action> {
        if !self.stream_active {
            return Vec::new();
        }
        let mut out = self.tick(now);
        let Some((artist, track)) = title.and_then(parse_icy_title) else {
            self.current = None;
            return out;
        };
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.meta.artist == artist && c.meta.title == track)
        {
            return out;
        }
        let meta = TrackMeta {
            artist,
            title: track,
            ..TrackMeta::default()
        };
        out.extend(self.begin(meta, true, now, unix));
        out
    }

    /// Re-align with an externally observed snapshot (startup, or after the
    /// event stream lagged).
    pub fn resync(
        &mut self,
        state: PlayerState,
        song: Option<&Song>,
        now: Instant,
        unix: i64,
    ) -> Vec<Action> {
        let key = song.map(|s| (s.id, s.path.as_str().to_owned()));
        let mut out = Vec::new();
        if key != self.song_key {
            out.extend(self.song_changed(song, now, unix));
        }
        out.extend(self.state_changed(state, now));
        out
    }

    fn check_listen(&mut self) -> Vec<Action> {
        let Some(cur) = self.current.as_mut() else {
            return Vec::new();
        };
        if cur.scrobbled || !cur.eligible() || cur.played < cur.threshold() {
            return Vec::new();
        }
        cur.scrobbled = true;
        vec![Action::Listen(Listen {
            meta: cur.meta.clone(),
            listened_at: cur.started_unix,
        })]
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use std::borrow::Cow;

    pub fn song(id: u64, path: &str, duration: Option<u64>, tags: &[(&'static str, &str)]) -> Song {
        Song {
            id,
            path: path.into(),
            duration: duration.map(Duration::from_secs),
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
                .map(|(k, v)| (Cow::Borrowed(*k), (*v).to_owned()))
                .collect(),
        }
    }

    pub fn tagged(id: u64, duration: Option<u64>) -> Song {
        song(
            id,
            &format!("music/{id}.flac"),
            duration,
            &[("artist", "Artist"), ("title", "Title"), ("album", "Album")],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    /// Drives the tracker with 1 s position ticks of simulated playback.
    struct Sim {
        t: ListenTracker,
        now: Instant,
        pos: Duration,
        out: Vec<Action>,
    }

    impl Sim {
        fn new() -> Self {
            Self {
                t: ListenTracker::new(TrackerSettings::default()),
                now: Instant::now(),
                pos: Duration::ZERO,
                out: Vec::new(),
            }
        }
        fn start(&mut self, song: &Song) {
            self.pos = Duration::ZERO;
            self.out
                .extend(self.t.song_changed(Some(song), self.now, 1000));
            self.out
                .extend(self.t.state_changed(PlayerState::Play, self.now));
        }
        fn play(&mut self, secs: u64) {
            for _ in 0..secs {
                self.now += SEC;
                self.pos += SEC;
                self.out.extend(self.t.position(self.pos, self.now));
            }
        }
        fn wall(&mut self, secs: u64) {
            self.now += Duration::from_secs(secs);
        }
        fn listens(&self) -> usize {
            self.out
                .iter()
                .filter(|a| matches!(a, Action::Listen(_)))
                .count()
        }
        fn now_playing(&self) -> usize {
            self.out
                .iter()
                .filter(|a| matches!(a, Action::NowPlaying(_)))
                .count()
        }
    }

    #[test]
    fn now_playing_on_start_and_listen_at_half() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(100)));
        assert_eq!(s.now_playing(), 1);
        s.play(49);
        assert_eq!(s.listens(), 0);
        s.play(1);
        assert_eq!(s.listens(), 1);
        s.play(30);
        assert_eq!(s.listens(), 1, "only one listen per play");
        match s.out.iter().find_map(|a| match a {
            Action::Listen(l) => Some(l),
            _ => None,
        }) {
            Some(l) => assert_eq!(l.listened_at, 1000),
            None => panic!("no listen"),
        }
    }

    #[test]
    fn long_track_caps_threshold_at_240s() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(1200)));
        s.play(239);
        assert_eq!(s.listens(), 0);
        s.play(1);
        assert_eq!(s.listens(), 1);
    }

    #[test]
    fn unknown_duration_uses_240s() {
        let mut s = Sim::new();
        s.start(&tagged(1, None));
        s.play(239);
        assert_eq!(s.listens(), 0);
        s.play(1);
        assert_eq!(s.listens(), 1);
    }

    #[test]
    fn short_tracks_are_never_scrobbled() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(29)));
        assert_eq!(s.now_playing(), 0);
        s.play(29);
        assert_eq!(s.listens(), 0);
        // exactly 30 s qualifies, threshold 15 s
        let mut s = Sim::new();
        s.start(&tagged(2, Some(30)));
        s.play(15);
        assert_eq!(s.listens(), 1);
    }

    #[test]
    fn pause_time_is_not_counted() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(100)));
        s.play(30);
        s.out.extend(s.t.state_changed(PlayerState::Pause, s.now));
        s.wall(600); // a long pause
        s.out.extend(s.t.state_changed(PlayerState::Play, s.now));
        s.play(10);
        assert_eq!(s.listens(), 0, "only 40 s actually played");
        assert_eq!(s.t.played(), Duration::from_secs(40));
        s.play(10);
        assert_eq!(s.listens(), 1);
        assert_eq!(s.now_playing(), 1, "resume does not re-send now-playing");
    }

    #[test]
    fn forward_seek_is_not_credited() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(300)));
        s.play(10);
        // Seek forward 100 s within one wall-clock second.
        s.now += SEC;
        s.pos += Duration::from_secs(100);
        s.out.extend(s.t.position(s.pos, s.now));
        assert_eq!(s.t.played(), Duration::from_secs(10));
        s.play(5);
        assert_eq!(s.t.played(), Duration::from_secs(15));
        assert_eq!(s.listens(), 0);
    }

    #[test]
    fn backward_seek_does_not_double_count() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(300)));
        s.play(20);
        s.now += SEC;
        s.pos = Duration::from_secs(5);
        s.out.extend(s.t.position(s.pos, s.now));
        assert_eq!(s.t.played(), Duration::from_secs(20));
        s.play(5);
        assert_eq!(s.t.played(), Duration::from_secs(25));
    }

    #[test]
    fn seek_while_paused_is_not_credited() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(300)));
        s.play(10);
        s.out.extend(s.t.state_changed(PlayerState::Pause, s.now));
        s.wall(5);
        s.out.extend(s.t.position(Duration::from_secs(200), s.now));
        s.out.extend(s.t.state_changed(PlayerState::Play, s.now));
        s.pos = Duration::from_secs(200);
        s.play(5);
        assert_eq!(s.t.played(), Duration::from_secs(15));
    }

    #[test]
    fn duplicate_song_changed_is_ignored_but_restart_is_new_track() {
        let mut s = Sim::new();
        let song = tagged(1, Some(100));
        s.start(&song);
        s.out.extend(s.t.song_changed(Some(&song), s.now, 1000));
        assert_eq!(s.now_playing(), 1);
        s.play(10);
        // Same song restarted later (repeat one): new track.
        s.out.extend(s.t.track_finished_and_restart(&song, s.now));
        assert_eq!(s.now_playing(), 2);
        assert_eq!(s.t.played(), Duration::ZERO);
    }

    #[test]
    fn song_change_resets_progress() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(100)));
        s.play(40);
        s.start(&tagged(2, Some(100)));
        assert_eq!(s.t.played(), Duration::ZERO);
        s.play(49);
        assert_eq!(s.listens(), 0);
        s.play(1);
        assert_eq!(s.listens(), 1);
        assert_eq!(s.now_playing(), 2);
    }

    #[test]
    fn untagged_songs_are_skipped() {
        let mut s = Sim::new();
        s.start(&song(1, "x/y.flac", Some(100), &[]));
        assert_eq!(s.now_playing(), 0);
        s.play(100);
        assert_eq!(s.listens(), 0);
    }

    #[test]
    fn started_paused_sends_now_playing_on_play() {
        let mut t = ListenTracker::new(TrackerSettings::default());
        let now = Instant::now();
        t.state_changed(PlayerState::Pause, now);
        assert!(
            t.song_changed(Some(&tagged(1, Some(100))), now, 5)
                .is_empty()
        );
        let out = t.state_changed(PlayerState::Play, now);
        assert!(matches!(out.as_slice(), [Action::NowPlaying(_)]));
    }

    #[test]
    fn stop_ends_the_track() {
        let mut s = Sim::new();
        s.start(&tagged(1, Some(100)));
        s.play(10);
        s.out.extend(s.t.state_changed(PlayerState::Stop, s.now));
        s.pos += Duration::from_secs(100);
        s.play(100);
        assert_eq!(s.listens(), 0);
    }

    fn stream_song() -> Song {
        song(9, "http://radio.example/stream", None, &[("name", "Radio")])
    }

    #[test]
    fn streams_ignored_by_default() {
        let mut t = ListenTracker::new(TrackerSettings::default());
        let mut now = Instant::now();
        t.state_changed(PlayerState::Play, now);
        t.song_changed(Some(&stream_song()), now, 1);
        assert!(t.stream_title(Some("A - B"), now, 1).is_empty());
        now += Duration::from_secs(300);
        assert!(t.tick(now).is_empty());
    }

    #[test]
    fn stream_titles_scrobble_after_60s() {
        let mut t = ListenTracker::new(TrackerSettings {
            scrobble_streams: true,
        });
        let mut now = Instant::now();
        t.state_changed(PlayerState::Play, now);
        assert!(t.song_changed(Some(&stream_song()), now, 1).is_empty());
        let out = t.stream_title(Some("Some Artist - Some Song"), now, 7);
        assert!(
            matches!(out.as_slice(), [Action::NowPlaying(m)] if m.artist == "Some Artist" && m.title == "Some Song")
        );
        now += Duration::from_secs(59);
        assert!(t.tick(now).is_empty());
        now += SEC;
        let out = t.tick(now);
        assert!(matches!(out.as_slice(), [Action::Listen(l)] if l.listened_at == 7));
        // Same title again: no new track.
        assert!(
            t.stream_title(Some("Some Artist - Some Song"), now, 8)
                .is_empty()
        );
        // New title starts fresh.
        let out = t.stream_title(Some("Other - Track"), now, 9);
        assert!(matches!(out.as_slice(), [Action::NowPlaying(_)]));
        assert_eq!(t.played(), Duration::ZERO);
        // Untitled / unparsable titles end the track.
        assert!(
            t.stream_title(Some("Just a station jingle"), now, 10)
                .is_empty()
        );
        now += Duration::from_secs(120);
        assert!(t.tick(now).is_empty());
    }

    #[test]
    fn icy_title_parsing() {
        assert_eq!(
            parse_icy_title("Artist - Title"),
            Some(("Artist".to_owned(), "Title".to_owned()))
        );
        assert_eq!(
            parse_icy_title("A - B - C"),
            Some(("A".to_owned(), "B - C".to_owned()))
        );
        assert_eq!(parse_icy_title("no separator"), None);
        assert_eq!(parse_icy_title(" - Title"), None);
        assert_eq!(parse_icy_title("Artist - "), None);
    }

    #[test]
    fn stream_path_detection() {
        assert!(is_stream_path("http://x/y"));
        assert!(is_stream_path("https://x/y"));
        assert!(!is_stream_path("file:///music/a.flac"));
        assert!(!is_stream_path("artist/album/a.flac"));
    }

    #[test]
    fn meta_from_song_picks_tags_and_mbids() {
        let song = song(
            1,
            "a.flac",
            Some(200),
            &[
                ("artist", " Band "),
                ("title", "Song"),
                ("album", "LP"),
                ("albumartist", "Band"),
                ("track", "3/12"),
                (
                    "musicbrainz_trackid",
                    "9F8B3C0E-1111-4222-8333-444455556666",
                ),
                ("musicbrainz_albumid", "not-a-uuid"),
                (
                    "musicbrainz_artistid",
                    "aaaaaaaa-1111-4222-8333-444455556666",
                ),
                (
                    "musicbrainz_artistid",
                    "bbbbbbbb-1111-4222-8333-444455556666",
                ),
            ],
        );
        let m = TrackMeta::from_song(&song).expect("meta");
        assert_eq!(m.artist, "Band");
        assert_eq!(m.track_number.as_deref(), Some("3"));
        assert_eq!(m.duration_secs, Some(200));
        assert_eq!(
            m.recording_mbid.as_deref(),
            Some("9f8b3c0e-1111-4222-8333-444455556666")
        );
        assert_eq!(m.release_mbid, None);
        assert_eq!(m.artist_mbids.len(), 2);
    }

    #[test]
    fn resync_aligns_with_snapshot() {
        let mut t = ListenTracker::new(TrackerSettings::default());
        let now = Instant::now();
        let song = tagged(4, Some(100));
        let out = t.resync(PlayerState::Play, Some(&song), now, 77);
        assert!(matches!(out.as_slice(), [Action::NowPlaying(_)]));
        // Same song again: nothing new.
        assert!(t.resync(PlayerState::Play, Some(&song), now, 78).is_empty());
        t.resync(PlayerState::Stop, None, now, 79);
        assert_eq!(t.played(), Duration::ZERO);
    }
}

#[cfg(test)]
impl ListenTracker {
    /// Test helper: simulate a natural end followed by the same song being
    /// started again well after the duplicate window.
    fn track_finished_and_restart(&mut self, song: &Song, now: Instant) -> Vec<Action> {
        self.track_finished();
        let later = now + DUPLICATE_WINDOW + Duration::from_secs(1);
        self.song_changed(Some(song), later, 2000)
    }
}
