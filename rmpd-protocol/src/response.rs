use rmpd_core::song::Song;
use rmpd_core::state::PlayerStatus;
use std::fmt::Write as FmtWrite;

/// Response type that can be either text or binary
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
        writeln!(self.buffer, "{}: {}", key, data.len()).unwrap();
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
        format!("ACK [{}@{}] {{{}}} {}\n", code, command_list_num, command, message)
    }

    pub fn field(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        writeln!(self.buffer, "{}: {}", key, value).unwrap();
        self
    }

    pub fn optional_field<T: std::fmt::Display>(&mut self, key: &str, value: Option<T>) -> &mut Self {
        if let Some(val) = value {
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

        self.field("playlist", status.playlist_version);
        self.field("playlistlength", status.playlist_length);
        self.field("mixrampdb", status.mixramp_db);

        let state_str = match status.state {
            rmpd_core::state::PlayerState::Stop => "stop",
            rmpd_core::state::PlayerState::Play => "play",
            rmpd_core::state::PlayerState::Pause => "pause",
        };
        self.field("state", state_str);

        if let Some(pos) = &status.current_song {
            self.field("song", pos.position);
            self.field("songid", pos.id);
        }

        // Show time and elapsed fields
        if let Some(elapsed) = status.elapsed {
            if let Some(duration) = status.duration {
                self.field("time", format!("{}:{}",
                    elapsed.as_secs(),
                    duration.as_secs()
                ));
            }
            self.field("elapsed", format!("{:.3}", elapsed.as_secs_f64()));
        }

        self.optional_field("duration", status.duration.map(|d| format!("{:.3}", d.as_secs_f64())));
        self.optional_field("bitrate", status.bitrate);

        if let Some(fmt) = status.audio_format {
            self.field("audio", format!("{}:{}:{}",
                fmt.sample_rate,
                fmt.bits_per_sample,
                fmt.channels
            ));
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

        if let Some(pos) = position {
            self.field("Pos", pos);
        }
        if let Some(song_id) = id {
            self.field("Id", song_id);
        }

        self.optional_field("Title", song.title.as_ref());
        self.optional_field("Artist", song.artist.as_ref());
        self.optional_field("Album", song.album.as_ref());
        self.optional_field("AlbumArtist", song.album_artist.as_ref());
        self.optional_field("Track", song.track);
        self.optional_field("Disc", song.disc);
        self.optional_field("Date", song.date.as_ref());
        self.optional_field("Genre", song.genre.as_ref());
        self.optional_field("Composer", song.composer.as_ref());
        self.optional_field("Performer", song.performer.as_ref());

        if let Some(duration) = song.duration {
            self.field("Time", duration.as_secs());
            self.field("duration", format!("{:.3}", duration.as_secs_f64()));
        }

        self
    }

    pub fn stats(&mut self, artists: u32, albums: u32, songs: u32, uptime: u64, db_playtime: u64, db_update: i64, playtime: u64) -> &mut Self {
        self.field("artists", artists);
        self.field("albums", albums);
        self.field("songs", songs);
        self.field("uptime", uptime);
        self.field("db_playtime", db_playtime);
        self.field("db_update", db_update);
        self.field("playtime", playtime);
        self
    }
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}
