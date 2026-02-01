use rmpd_core::queue::Queue;
use rmpd_core::song::Song;
use rmpd_core::state::{ConsumeMode, PlayerState, PlayerStatus, QueuePosition, SingleMode};
use rmpd_protocol::statefile::StateFile;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

/// Helper for managing temporary state files in tests
pub struct TempStateFile {
    pub path: PathBuf,
    _temp_dir: TempDir,
}

impl TempStateFile {
    pub fn new(content: &str) -> Self {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state");
        std::fs::write(&path, content).unwrap();

        Self {
            path,
            _temp_dir: temp_dir,
        }
    }

    pub fn new_empty() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("state");

        Self {
            path,
            _temp_dir: temp_dir,
        }
    }

    pub fn path_str(&self) -> String {
        self.path.to_str().unwrap().to_string()
    }

    pub fn read_content(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

/// Fluent builder for creating PlayerStatus instances in tests
pub struct StatusBuilder {
    volume: u8,
    state: PlayerState,
    current_song: Option<QueuePosition>,
    next_song: Option<QueuePosition>,
    elapsed: Option<Duration>,
    duration: Option<Duration>,
    bitrate: Option<u32>,
    audio_format: Option<rmpd_core::song::AudioFormat>,
    random: bool,
    repeat: bool,
    single: SingleMode,
    consume: ConsumeMode,
    crossfade: u32,
    mixramp_db: f32,
    mixramp_delay: f32,
}

impl StatusBuilder {
    pub fn new() -> Self {
        Self {
            volume: 100,
            state: PlayerState::Stop,
            current_song: None,
            next_song: None,
            elapsed: None,
            duration: None,
            bitrate: None,
            audio_format: None,
            random: false,
            repeat: false,
            single: SingleMode::Off,
            consume: ConsumeMode::Off,
            crossfade: 0,
            mixramp_db: 0.0,
            mixramp_delay: -1.0,
        }
    }

    pub fn volume(mut self, volume: u8) -> Self {
        self.volume = volume;
        self
    }

    pub fn state(mut self, state: PlayerState) -> Self {
        self.state = state;
        self
    }

    pub fn current_position(mut self, position: u32, id: u32) -> Self {
        self.current_song = Some(QueuePosition { position, id });
        self
    }

    pub fn elapsed(mut self, secs: u64) -> Self {
        self.elapsed = Some(Duration::from_secs(secs));
        self
    }

    pub fn random(mut self, enabled: bool) -> Self {
        self.random = enabled;
        self
    }

    pub fn repeat(mut self, enabled: bool) -> Self {
        self.repeat = enabled;
        self
    }

    pub fn single(mut self, mode: SingleMode) -> Self {
        self.single = mode;
        self
    }

    pub fn consume(mut self, mode: ConsumeMode) -> Self {
        self.consume = mode;
        self
    }

    pub fn crossfade(mut self, seconds: u32) -> Self {
        self.crossfade = seconds;
        self
    }

    pub fn mixramp_db(mut self, db: f32) -> Self {
        self.mixramp_db = db;
        self
    }

    pub fn mixramp_delay(mut self, delay: f32) -> Self {
        self.mixramp_delay = delay;
        self
    }

    pub fn build(self, playlist_length: u32) -> PlayerStatus {
        PlayerStatus {
            volume: self.volume,
            state: self.state,
            current_song: self.current_song,
            next_song: self.next_song,
            elapsed: self.elapsed,
            duration: self.duration,
            bitrate: self.bitrate,
            audio_format: self.audio_format,
            random: self.random,
            repeat: self.repeat,
            single: self.single,
            consume: self.consume,
            crossfade: self.crossfade,
            mixramp_db: self.mixramp_db,
            mixramp_delay: self.mixramp_delay,
            playlist_version: 1,
            playlist_length,
            updating_db: None,
            error: None,
        }
    }
}

impl Default for StatusBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create test songs
pub fn create_test_song(path: &str, track: u32) -> Song {
    Song {
        id: track as u64,
        path: path.into(),
        duration: Some(Duration::from_secs(180)),
        title: Some(format!("Track {track}")),
        artist: Some("Test Artist".to_string()),
        album: Some("Test Album".to_string()),
        album_artist: None,
        track: Some(track),
        disc: None,
        date: Some("2024".to_string()),
        genre: Some("Rock".to_string()),
        composer: None,
        performer: None,
        comment: None,
        musicbrainz_trackid: None,
        musicbrainz_albumid: None,
        musicbrainz_artistid: None,
        musicbrainz_albumartistid: None,
        musicbrainz_releasegroupid: None,
        musicbrainz_releasetrackid: None,
        artist_sort: None,
        album_artist_sort: None,
        original_date: None,
        label: None,
        sample_rate: Some(44100),
        channels: Some(2),
        bits_per_sample: Some(16),
        bitrate: Some(320),
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
    }
}

/// Helper to create a queue with test songs
pub fn create_test_queue(num_songs: u32) -> Queue {
    let mut queue = Queue::new();
    for i in 0..num_songs {
        queue.add(create_test_song(&format!("/music/song{i}.mp3"), i));
    }
    queue
}

/// Helper to save and load a state file
pub async fn save_and_load(
    status: &PlayerStatus,
    queue: &Queue,
) -> Result<rmpd_protocol::statefile::SavedState, rmpd_core::error::RmpdError> {
    let temp = TempStateFile::new_empty();
    let statefile = StateFile::new(temp.path_str());

    statefile.save(status, queue).await?;
    statefile
        .load()?
        .ok_or_else(|| rmpd_core::error::RmpdError::Library("No state loaded".to_string()))
}
