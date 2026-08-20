use rmpd_core::song::Song;
use rmpd_core::state::PlayerStatus;
use std::fmt::Write as FmtWrite;

/// Database statistics
pub struct Stats {
    pub artists: u32,
    pub albums: u32,
    pub songs: u32,
    pub uptime: u64,
    pub db_playtime: u64,
    pub db_update: i64,
    pub playtime: u64,
}

/// Response type that can be either text or binary
#[derive(Debug)]
pub enum Response {
    Text(String),
    Binary(Vec<u8>),
}

impl Response {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Response::Text(s) => s.as_bytes(),
            Response::Binary(b) => b.as_slice(),
        }
    }
}

impl From<String> for Response {
    fn from(s: String) -> Self {
        Response::Text(s)
    }
}

impl From<Vec<u8>> for Response {
    fn from(b: Vec<u8>) -> Self {
        Response::Binary(b)
    }
}

#[derive(Debug)]
pub struct ResponseBuilder {
    buffer: String,
    binary_data: Option<Vec<u8>>,
}

impl ResponseBuilder {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(4096),
            binary_data: None,
        }
    }

    /// Clear the buffer for reuse without deallocating.
    /// Useful for reusing the builder across multiple responses.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.binary_data = None;
    }

    pub fn ok(mut self) -> String {
        // If we have binary data, we need to handle it differently
        // For now, just append OK (binary handling will need special treatment)
        self.buffer.push_str("OK\n");
        self.buffer
    }

    pub fn binary_field(&mut self, key: &str, data: &[u8]) -> &mut Self {
        // Store binary data for later
        // The actual binary response format is: "binary: <length>\n<data>OK\n"
        writeln!(self.buffer, "{}: {}", key, data.len())
            .expect("writing to String buffer cannot fail");
        self.binary_data = Some(data.to_vec());
        self
    }

    pub fn to_bytes(self) -> Vec<u8> {
        let mut result = self.buffer.into_bytes();
        if let Some(binary) = self.binary_data {
            result.extend_from_slice(&binary);
        }
        // Don't add extra newline here - it's handled by ok() or caller
        result
    }

    pub fn to_binary_response(self) -> Vec<u8> {
        let mut result = self.buffer.into_bytes();
        if let Some(binary) = self.binary_data {
            result.extend_from_slice(&binary);
        }
        result.extend_from_slice(b"\nOK\n");
        result
    }

    pub fn error(code: i32, command_list_num: i32, command: &str, message: &str) -> String {
        format!("ACK [{code}@{command_list_num}] {{{command}}} {message}\n")
    }

    pub fn field(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        writeln!(self.buffer, "{key}: {value}").expect("writing to String buffer cannot fail");
        self
    }

    pub fn optional_field<T: std::fmt::Display>(
        &mut self,
        key: &str,
        value: Option<T>,
    ) -> &mut Self {
        if let Some(val) = value {
            self.field(key, val);
        }
        self
    }

    /// Add an optional string field, skipping None and empty strings.
    /// MPD omits tags entirely when their value is empty.
    pub fn optional_str_field(&mut self, key: &str, value: Option<&String>) -> &mut Self {
        if let Some(val) = value
            && !val.is_empty()
        {
            self.field(key, val);
        }
        self
    }

    /// Add a blank line to separate entities in the response
    pub fn blank_line(&mut self) -> &mut Self {
        self.buffer.push('\n');
        self
    }

    pub fn status(
        &mut self,
        status: &PlayerStatus,
        partition: &str,
        last_loaded_playlist: &str,
    ) -> &mut Self {
        self.field("volume", status.volume);
        self.field("repeat", if status.repeat { 1 } else { 0 });
        self.field("random", if status.random { 1 } else { 0 });

        let single_val = match status.single {
            rmpd_core::state::SingleMode::Off => "0",
            rmpd_core::state::SingleMode::On => "1",
            rmpd_core::state::SingleMode::Oneshot => "oneshot",
        };
        self.field("single", single_val);

        let consume_val = match status.consume {
            rmpd_core::state::ConsumeMode::Off => "0",
            rmpd_core::state::ConsumeMode::On => "1",
            rmpd_core::state::ConsumeMode::Oneshot => "oneshot",
        };
        self.field("consume", consume_val);

        self.field("partition", partition);

        self.field("playlist", status.playlist_version);
        self.field("playlistlength", status.playlist_length);
        self.field("mixrampdb", status.mixramp_db);

        let state_str = match status.state {
            rmpd_core::state::PlayerState::Stop => "stop",
            rmpd_core::state::PlayerState::Play => "play",
            rmpd_core::state::PlayerState::Pause => "pause",
        };
        self.field("state", state_str);
        self.field("lastloadedplaylist", last_loaded_playlist);

        // MPD order: xfade then mixrampdelay, both only when non-default
        // (PlayerCommands.cxx handle_status).
        if status.crossfade > 0 {
            self.field("xfade", status.crossfade);
        }
        if status.mixramp_delay > 0.0 {
            self.field("mixrampdelay", status.mixramp_delay);
        }

        if let Some(pos) = &status.current_song {
            self.field("song", pos.position);
            self.field("songid", pos.id);
        }

        // time/elapsed/bitrate/duration/audio only when playing or paused
        // (not stopped). time/elapsed/bitrate are unconditional in that
        // state per MPD; duration and audio are individually optional.
        if !matches!(status.state, rmpd_core::state::PlayerState::Stop) {
            let elapsed = status.elapsed.unwrap_or_default();
            let duration_secs = status.duration.map(|d| d.as_secs()).unwrap_or(0);
            self.field("time", format!("{}:{duration_secs}", elapsed.as_secs()));
            self.field("elapsed", format!("{:.3}", elapsed.as_secs_f64()));
            self.field("bitrate", status.bitrate.unwrap_or(0));
            self.optional_field(
                "duration",
                status.duration.map(|d| format!("{:.3}", d.as_secs_f64())),
            );

            if let Some(fmt) = status.audio_format {
                self.field(
                    "audio",
                    format!(
                        "{}:{}:{}",
                        fmt.sample_rate, fmt.bits_per_sample, fmt.channels
                    ),
                );
            }
        }

        self.optional_field("updating_db", status.updating_db);
        self.optional_field("error", status.error.as_ref());

        // nextsong/nextsongid are printed last, matching handle_status, and
        // only when there is a current song: MPD derives them from
        // playlist::GetNextPosition(), which returns -1 whenever
        // `current < 0` (Playlist.cxx:320), so a stopped player never
        // reports a next song.
        if let (Some(_), Some(next)) = (&status.current_song, &status.next_song) {
            self.field("nextsong", next.position);
            self.field("nextsongid", next.id);
        }

        self
    }

    pub fn song(
        &mut self,
        song: &Song,
        position: Option<u32>,
        id: Option<u32>,
        range: Option<(f64, f64)>,
    ) -> &mut Self {
        self.field("file", &song.path);
        // MPD order (SongPrint.cxx song_print_info): Range, Last-Modified,
        // Added, Format, tags in file insertion order, Time/duration, then
        // Pos/Id/Prio appended by the caller (queue/Print.cxx).
        if let Some((start, end)) = range {
            if end > 0.0 {
                self.field("Range", format!("{start:.3}-{end:.3}"));
            } else if start > 0.0 {
                self.field("Range", format!("{start:.3}-"));
            }
        }
        if song.last_modified > 0 {
            let ts = crate::commands::utils::format_iso8601_timestamp(song.last_modified);
            self.field("Last-Modified", &ts);
        }
        if song.added_at > 0 {
            let ts = crate::commands::utils::format_iso8601_timestamp(song.added_at);
            self.field("Added", &ts);
        }
        // Format: samplerate:bits:channels — before tags (matching MPD's SongPrint.cxx order)
        if let Some(sr) = song.sample_rate {
            let bits = match song.bits_per_sample {
                Some(0) | None => "f".to_string(),
                Some(b) => b.to_string(),
            };
            let ch = song.channels.unwrap_or(2);
            self.field("Format", format!("{}:{}:{}", sr, bits, ch));
        }
        // Tags in file insertion order (matching MPD which outputs tags as stored in the file).
        // Comment is excluded from default tag mask (MPD's Settings.cxx: All & ~TAG_COMMENT)
        for (tag, value) in &song.tags {
            if tag == "comment" || value.is_empty() {
                continue;
            }
            let canonical = rmpd_core::song::canonical_tag_name(tag);
            self.field(canonical, value);
        }
        // Duration
        if let Some(duration) = song.duration {
            self.field("Time", duration.as_millis().saturating_add(500) / 1000);
            self.field("duration", format!("{:.3}", duration.as_secs_f64()));
        }
        // Queue position/id (at the end, matching MPD)
        if let Some(pos) = position {
            self.field("Pos", pos);
        }
        if let Some(song_id) = id {
            self.field("Id", song_id);
        }
        self
    }

    pub fn stats(&mut self, stats: &Stats) -> &mut Self {
        self.field("uptime", stats.uptime);
        self.field("playtime", stats.playtime);
        self.field("artists", stats.artists);
        self.field("albums", stats.albums);
        self.field("songs", stats.songs);
        self.field("db_playtime", stats.db_playtime);
        // MPD omits db_update when the database has never been updated
        // (db.GetUpdateStamp() negative -> Stats.cxx db_stats_print).
        if stats.db_update > 0 {
            self.field("db_update", stats.db_update);
        }
        self
    }
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::song::Song;

    /// A synced source song carrying sample-rate / bit-depth / channel-count,
    /// shaped like `map_song`'s output.
    fn source_song() -> Song {
        Song {
            id: 0,
            path: "alarm-music/Artist/Album/song-1.flac".into(),
            duration: Some(std::time::Duration::from_secs(240)),
            sample_rate: Some(48_000),
            channels: Some(2),
            bits_per_sample: Some(24),
            bitrate: Some(960),
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
            tags: vec![(
                rmpd_core::song::intern_tag_key("title"),
                "Echoes".to_string(),
            )],
        }
    }

    /// The source song emits the MPD `Format: <rate>:<bits>:<channels>` line,
    /// and its mount-style `file` path keeps the codec-revealing `.flac` suffix.
    #[test]
    fn source_song_emits_format_line_and_keeps_extension() {
        let mut rb = ResponseBuilder::new();
        rb.song(&source_song(), None, None, None);
        let out = rb.ok();

        assert!(
            out.contains("Format: 48000:24:2"),
            "expected 'Format: 48000:24:2' in response, got:\n{out}"
        );
        assert!(
            out.contains("file: alarm-music/Artist/Album/song-1.flac"),
            "expected mount-style file line with .flac extension, got:\n{out}"
        );
    }

    fn base_status() -> PlayerStatus {
        PlayerStatus::default()
    }

    /// Regression: `mixrampdelay` and `xfade` must be emitted after `state`
    /// and `lastloadedplaylist`, and `nextsong`/`nextsongid` must be the
    /// very last fields — matching PlayerCommands.cxx handle_status, not
    /// the previous interleaved order.
    #[test]
    fn status_field_order_matches_mpd() {
        let mut status = base_status();
        status.state = rmpd_core::state::PlayerState::Play;
        status.crossfade = 5;
        status.mixramp_delay = 2.5;
        status.current_song = Some(rmpd_core::state::QueuePosition { position: 0, id: 1 });
        status.next_song = Some(rmpd_core::state::QueuePosition { position: 1, id: 2 });
        status.elapsed = Some(std::time::Duration::from_secs(10));
        status.duration = Some(std::time::Duration::from_secs(200));
        status.bitrate = Some(320);
        status.updating_db = Some(3);
        status.error = Some("boom".to_string());

        let mut rb = ResponseBuilder::new();
        rb.status(&status, "default", "");
        let out = rb.ok();

        let idx = |name: &str| {
            out.find(&format!("{name}:"))
                .unwrap_or_else(|| panic!("missing '{name}' in:\n{out}"))
        };

        assert!(idx("state") < idx("xfade"), "state before xfade:\n{out}");
        assert!(
            idx("xfade") < idx("mixrampdelay"),
            "xfade before mixrampdelay:\n{out}"
        );
        assert!(
            idx("mixrampdelay") < idx("song"),
            "mixrampdelay before song:\n{out}"
        );
        assert!(idx("song") < idx("time"), "song before time:\n{out}");
        assert!(idx("time") < idx("bitrate"), "time before bitrate:\n{out}");
        assert!(
            idx("bitrate") < idx("duration"),
            "bitrate before duration:\n{out}"
        );
        assert!(
            idx("error") < idx("nextsong"),
            "error before nextsong:\n{out}"
        );
        assert!(
            idx("nextsong") < idx("nextsongid"),
            "nextsong before nextsongid:\n{out}"
        );
    }

    /// While playing/paused, MPD always emits `time`/`elapsed`/`bitrate`
    /// (defaulting duration to 0, bitrate to 0) even when the decoder hasn't
    /// reported them yet; `duration` itself stays omitted when unknown.
    #[test]
    fn status_playing_defaults_time_and_bitrate_when_unknown() {
        let mut status = base_status();
        status.state = rmpd_core::state::PlayerState::Play;
        status.elapsed = Some(std::time::Duration::from_secs(5));
        status.duration = None;
        status.bitrate = None;

        let mut rb = ResponseBuilder::new();
        rb.status(&status, "default", "");
        let out = rb.ok();

        assert!(out.contains("time: 5:0\n"), "got:\n{out}");
        assert!(out.contains("elapsed: 5.000\n"), "got:\n{out}");
        assert!(out.contains("bitrate: 0\n"), "got:\n{out}");
        assert!(!out.contains("duration:"), "got:\n{out}");
    }

    /// Stopped state omits every playback-only field.
    #[test]
    fn status_stopped_omits_playback_fields() {
        let status = base_status();
        let mut rb = ResponseBuilder::new();
        rb.status(&status, "default", "");
        let out = rb.ok();

        for field in [
            "time:",
            "elapsed:",
            "bitrate:",
            "duration:",
            "audio:",
            "song:",
            "songid:",
            "nextsong:",
            "xfade:",
            "mixrampdelay:",
            "updating_db:",
            "error:",
        ] {
            assert!(
                !out.contains(field),
                "unexpected '{field}' when stopped:\n{out}"
            );
        }
    }

    /// `Range` is positioned right after `file` (SongPrint.cxx PrintRange),
    /// and formats an open-ended range without a trailing bound.
    #[test]
    fn song_range_field_placement_and_format() {
        let mut rb = ResponseBuilder::new();
        rb.song(&source_song(), Some(3), Some(7), Some((12.5, 60.0)));
        let out = rb.ok();
        assert!(out.contains("Range: 12.500-60.000\n"), "got:\n{out}");
        let file_idx = out.find("file:").unwrap();
        let range_idx = out.find("Range:").unwrap();
        let format_idx = out.find("Format:").unwrap();
        assert!(
            file_idx < range_idx && range_idx < format_idx,
            "got:\n{out}"
        );

        let mut rb2 = ResponseBuilder::new();
        rb2.song(&source_song(), None, None, Some((12.5, 0.0)));
        let out2 = rb2.ok();
        assert!(out2.contains("Range: 12.500-\n"), "got:\n{out2}");

        let mut rb3 = ResponseBuilder::new();
        rb3.song(&source_song(), None, None, None);
        let out3 = rb3.ok();
        assert!(!out3.contains("Range:"), "got:\n{out3}");
    }

    /// MPD omits `db_update` entirely when the database has never been
    /// updated (Stats.cxx db_stats_print: `IsNegative(update_stamp)`).
    #[test]
    fn stats_omits_db_update_when_never_updated() {
        let stats = Stats {
            artists: 0,
            albums: 0,
            songs: 0,
            uptime: 10,
            db_playtime: 0,
            db_update: 0,
            playtime: 0,
        };
        let mut rb = ResponseBuilder::new();
        rb.stats(&stats);
        let out = rb.ok();
        assert!(!out.contains("db_update"), "got:\n{out}");
    }

    #[test]
    fn stats_includes_db_update_when_present() {
        let stats = Stats {
            artists: 1,
            albums: 1,
            songs: 1,
            uptime: 10,
            db_playtime: 100,
            db_update: 1_700_000_000,
            playtime: 0,
        };
        let mut rb = ResponseBuilder::new();
        rb.stats(&stats);
        let out = rb.ok();
        assert!(out.contains("db_update: 1700000000\n"), "got:\n{out}");
    }
}
