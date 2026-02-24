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
            buffer: String::new(),
            binary_data: None,
        }
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

    pub fn status(&mut self, status: &PlayerStatus) -> &mut Self {
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

        self.field("partition", "default");

        self.field("playlist", status.playlist_version);
        self.field("playlistlength", status.playlist_length);
        self.field("mixrampdb", status.mixramp_db);
        if status.mixramp_delay > 0.0 {
            self.field("mixrampdelay", status.mixramp_delay);
        }

        let state_str = match status.state {
            rmpd_core::state::PlayerState::Stop => "stop",
            rmpd_core::state::PlayerState::Play => "play",
            rmpd_core::state::PlayerState::Pause => "pause",
        };
        self.field("state", state_str);
        self.field("lastloadedplaylist", "");

        if let Some(pos) = &status.current_song {
            self.field("song", pos.position);
            self.field("songid", pos.id);
        }

        // Show time and elapsed fields
        if let Some(elapsed) = status.elapsed {
            if let Some(duration) = status.duration {
                self.field(
                    "time",
                    format!("{}:{}", elapsed.as_secs(), duration.as_secs()),
                );
            }
            self.field("elapsed", format!("{:.3}", elapsed.as_secs_f64()));
        }

        self.optional_field(
            "duration",
            status.duration.map(|d| format!("{:.3}", d.as_secs_f64())),
        );
        self.optional_field("bitrate", status.bitrate);

        if let Some(fmt) = status.audio_format {
            self.field(
                "audio",
                format!(
                    "{}:{}:{}",
                    fmt.sample_rate, fmt.bits_per_sample, fmt.channels
                ),
            );
        }

        if let Some(next) = &status.next_song {
            self.field("nextsong", next.position);
            self.field("nextsongid", next.id);
        }

        if status.crossfade > 0 {
            self.field("xfade", status.crossfade);
        }

        self.optional_field("updating_db", status.updating_db);
        self.optional_field("error", status.error.as_ref());

        self
    }

    pub fn song(&mut self, song: &Song, position: Option<u32>, id: Option<u32>) -> &mut Self {
        self.field("file", &song.path);
        // MPD order: Last-Modified, Added, Format, tags in file insertion order, Time/duration, Pos/Id
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
        self.field("db_update", stats.db_update);
        self
    }
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}
