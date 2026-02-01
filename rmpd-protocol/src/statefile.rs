use rmpd_core::error::Result;
use rmpd_core::queue::Queue;
use rmpd_core::state::{PlayerState, PlayerStatus};
use std::fs;
use std::path::Path;
use tracing::{debug, info};

/// Save and restore MPD-compatible state file
#[derive(Debug)]
pub struct StateFile {
    path: String,
}

impl StateFile {
    pub fn new(path: String) -> Self {
        Self { path }
    }

    /// Save current state to file
    pub async fn save(&self, status: &PlayerStatus, queue: &Queue) -> Result<()> {
        let mut content = String::new();

        // Volume (sw_volume for software volume)
        content.push_str(&format!("sw_volume: {}\n", status.volume));

        // Playback state
        let state_str = match status.state {
            PlayerState::Stop => "stop",
            PlayerState::Play => "play",
            PlayerState::Pause => "pause",
        };
        content.push_str(&format!("state: {state_str}\n"));

        // Current song position
        if let Some(current) = &status.current_song {
            content.push_str(&format!("current: {}\n", current.position));
        }

        // Playback time (elapsed)
        if let Some(elapsed) = &status.elapsed {
            content.push_str(&format!("time: {:.6}\n", elapsed.as_secs_f64()));
        }

        // Playback options
        content.push_str(&format!("random: {}\n", if status.random { 1 } else { 0 }));
        content.push_str(&format!("repeat: {}\n", if status.repeat { 1 } else { 0 }));

        let single_val = match status.single {
            rmpd_core::state::SingleMode::Off => 0,
            rmpd_core::state::SingleMode::On => 1,
            rmpd_core::state::SingleMode::Oneshot => 2,
        };
        content.push_str(&format!("single: {single_val}\n"));

        let consume_val = match status.consume {
            rmpd_core::state::ConsumeMode::Off => 0,
            rmpd_core::state::ConsumeMode::On => 1,
            rmpd_core::state::ConsumeMode::Oneshot => 2,
        };
        content.push_str(&format!("consume: {consume_val}\n"));

        // Crossfade and mixramp
        content.push_str(&format!("crossfade: {}\n", status.crossfade));
        content.push_str(&format!("mixrampdb: {:.6}\n", status.mixramp_db));
        content.push_str(&format!("mixrampdelay: {:.6}\n", status.mixramp_delay));

        // Playlist
        content.push_str("playlist_begin\n");
        for item in queue.items() {
            content.push_str(&format!("{}:{}\n", item.position, item.song.path));
        }
        content.push_str("playlist_end\n");

        // Write to file atomically (write to temp, then rename)
        let temp_path = format!("{}.tmp", self.path);
        fs::write(&temp_path, content)?;
        fs::rename(&temp_path, &self.path)?;

        info!("State saved to {}", self.path);
        Ok(())
    }

    /// Load state from file
    pub fn load(&self) -> Result<Option<SavedState>> {
        let path = Path::new(&self.path);
        if !path.exists() {
            debug!("State file not found: {}", self.path);
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;

        let mut state = SavedState::default();
        let mut in_playlist = false;
        let mut playlist_items = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line == "playlist_begin" {
                in_playlist = true;
                continue;
            } else if line == "playlist_end" {
                in_playlist = false;
                continue;
            }

            if in_playlist {
                // Parse playlist item: "position:path"
                if let Some((_pos_str, path)) = line.split_once(':') {
                    playlist_items.push(path.to_string());
                }
            } else {
                // Parse key: value pairs
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim();

                    match key {
                        "sw_volume" => {
                            state.volume = value.parse().unwrap_or(100);
                        }
                        "state" => {
                            state.state = match value {
                                "play" => Some(PlayerState::Play),
                                "pause" => Some(PlayerState::Pause),
                                "stop" => Some(PlayerState::Stop),
                                _ => None,
                            };
                        }
                        "current" => {
                            state.current_position = value.parse().ok();
                        }
                        "time" => {
                            state.elapsed_seconds = value.parse().ok();
                        }
                        "random" => {
                            state.random = value == "1";
                        }
                        "repeat" => {
                            state.repeat = value == "1";
                        }
                        "single" => {
                            state.single = match value {
                                "1" => rmpd_core::state::SingleMode::On,
                                "2" => rmpd_core::state::SingleMode::Oneshot,
                                _ => rmpd_core::state::SingleMode::Off,
                            };
                        }
                        "consume" => {
                            state.consume = match value {
                                "1" => rmpd_core::state::ConsumeMode::On,
                                "2" => rmpd_core::state::ConsumeMode::Oneshot,
                                _ => rmpd_core::state::ConsumeMode::Off,
                            };
                        }
                        "crossfade" => {
                            state.crossfade = value.parse().unwrap_or(0);
                        }
                        "mixrampdb" => {
                            state.mixramp_db = value.parse().unwrap_or(0.0);
                        }
                        "mixrampdelay" => {
                            state.mixramp_delay = value.parse().unwrap_or(-1.0);
                        }
                        _ => {} // Ignore unknown keys
                    }
                }
            }
        }

        state.playlist_paths = playlist_items;
        info!("State loaded from {}", self.path);
        Ok(Some(state))
    }
}

/// State loaded from file
#[derive(Debug, Default)]
pub struct SavedState {
    pub volume: u8,
    pub state: Option<PlayerState>,
    pub current_position: Option<u32>,
    pub elapsed_seconds: Option<f64>,
    pub random: bool,
    pub repeat: bool,
    pub single: rmpd_core::state::SingleMode,
    pub consume: rmpd_core::state::ConsumeMode,
    pub crossfade: u32,
    pub mixramp_db: f32,
    pub mixramp_delay: f32,
    pub playlist_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpd_core::queue::Queue;
    use rmpd_core::song::Song;
    use rmpd_core::state::{ConsumeMode, PlayerState, PlayerStatus, QueuePosition, SingleMode};
    use std::time::Duration;
    use tempfile::TempDir;

    fn create_test_song(path: &str, position: u32) -> Song {
        Song {
            id: position as u64,
            path: path.into(),
            duration: Some(Duration::from_secs(180)),
            title: Some(format!("Song {position}")),
            artist: Some("Test Artist".to_string()),
            album: Some("Test Album".to_string()),
            album_artist: None,
            track: Some(position),
            disc: None,
            date: None,
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

    #[tokio::test]
    async fn test_save_and_load_basic() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let mut queue = Queue::new();
        queue.add(create_test_song("/music/song1.mp3", 0));
        queue.add(create_test_song("/music/song2.mp3", 1));

        let status = PlayerStatus {
            volume: 75,
            state: PlayerState::Play,
            current_song: Some(QueuePosition {
                position: 0,
                id: 0,
            }),
            next_song: None,
            elapsed: Some(Duration::from_secs(42)),
            duration: Some(Duration::from_secs(180)),
            bitrate: Some(320),
            audio_format: Some(rmpd_core::song::AudioFormat {
                sample_rate: 44100,
                channels: 2,
                bits_per_sample: 16,
            }),
            random: false,
            repeat: true,
            single: SingleMode::Off,
            consume: ConsumeMode::Off,
            crossfade: 0,
            mixramp_db: 0.0,
            mixramp_delay: -1.0,
            playlist_version: 1,
            playlist_length: 2,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();

        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.volume, 75);
        assert_eq!(loaded.state, Some(PlayerState::Play));
        assert_eq!(loaded.current_position, Some(0));
        assert_eq!(loaded.elapsed_seconds, Some(42.0));
        assert!(loaded.repeat);
        assert!(!loaded.random);
        assert_eq!(loaded.playlist_paths.len(), 2);
        assert_eq!(loaded.playlist_paths[0], "/music/song1.mp3");
        assert_eq!(loaded.playlist_paths[1], "/music/song2.mp3");
    }

    #[tokio::test]
    async fn test_save_and_load_all_playback_modes() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let queue = Queue::new();

        // Test Play state
        let mut status = PlayerStatus {
            volume: 100,
            state: PlayerState::Play,
            current_song: None,
            next_song: None,
            elapsed: None,
            duration: None,
            bitrate: None,
            audio_format: None,
            random: true,
            repeat: true,
            single: SingleMode::On,
            consume: ConsumeMode::Oneshot,
            crossfade: 5,
            mixramp_db: -17.0,
            mixramp_delay: 2.0,
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.state, Some(PlayerState::Play));
        assert!(loaded.random);
        assert!(loaded.repeat);
        assert_eq!(loaded.single, SingleMode::On);
        assert_eq!(loaded.consume, ConsumeMode::Oneshot);
        assert_eq!(loaded.crossfade, 5);
        assert_eq!(loaded.mixramp_db, -17.0);
        assert_eq!(loaded.mixramp_delay, 2.0);

        // Test Pause state
        status.state = PlayerState::Pause;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.state, Some(PlayerState::Pause));

        // Test Stop state
        status.state = PlayerState::Stop;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.state, Some(PlayerState::Stop));
    }

    #[tokio::test]
    async fn test_save_and_load_single_modes() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let queue = Queue::new();
        let mut status = PlayerStatus {
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
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        // Test SingleMode::Off
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.single, SingleMode::Off);

        // Test SingleMode::On
        status.single = SingleMode::On;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.single, SingleMode::On);

        // Test SingleMode::Oneshot
        status.single = SingleMode::Oneshot;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.single, SingleMode::Oneshot);
    }

    #[tokio::test]
    async fn test_save_and_load_consume_modes() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let queue = Queue::new();
        let mut status = PlayerStatus {
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
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        // Test ConsumeMode::Off
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.consume, ConsumeMode::Off);

        // Test ConsumeMode::On
        status.consume = ConsumeMode::On;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.consume, ConsumeMode::On);

        // Test ConsumeMode::Oneshot
        status.consume = ConsumeMode::Oneshot;
        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.consume, ConsumeMode::Oneshot);
    }

    #[tokio::test]
    async fn test_empty_queue() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let queue = Queue::new();
        let status = PlayerStatus {
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
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.playlist_paths.len(), 0);
    }

    #[tokio::test]
    async fn test_large_queue() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let mut queue = Queue::new();
        for i in 0..1000 {
            queue.add(create_test_song(&format!("/music/song{i}.mp3"), i));
        }

        let status = PlayerStatus {
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
            playlist_version: 1,
            playlist_length: 1000,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.playlist_paths.len(), 1000);
        assert_eq!(loaded.playlist_paths[0], "/music/song0.mp3");
        assert_eq!(loaded.playlist_paths[999], "/music/song999.mp3");
    }

    #[tokio::test]
    async fn test_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path.clone());

        let queue = Queue::new();
        let status = PlayerStatus {
            volume: 50,
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
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();

        // Verify temp file doesn't exist
        let temp_path = format!("{state_path}.tmp");
        assert!(!std::path::Path::new(&temp_path).exists());

        // Verify main file exists
        assert!(std::path::Path::new(&state_path).exists());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir
            .path()
            .join("nonexistent")
            .to_str()
            .unwrap()
            .to_string();
        let statefile = StateFile::new(state_path);

        let result = statefile.load().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_malformed_volume() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(&state_path, "sw_volume: invalid\n").unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.volume, 100); // Should default to 100
    }

    #[test]
    fn test_parse_unknown_keys() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(
            &state_path,
            "sw_volume: 80\nfuture_key: some_value\nstate: play\n",
        )
        .unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.volume, 80);
        assert_eq!(loaded.state, Some(PlayerState::Play));
    }

    #[test]
    fn test_parse_invalid_state() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(&state_path, "state: invalid\n").unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.state, None);
    }

    #[test]
    fn test_parse_playlist_with_colons() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(
            &state_path,
            "sw_volume: 100\nplaylist_begin\n0:/music/artist:album/song.mp3\nplaylist_end\n",
        )
        .unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.playlist_paths.len(), 1);
        assert_eq!(loaded.playlist_paths[0], "/music/artist:album/song.mp3");
    }

    #[test]
    fn test_parse_empty_lines() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(
            &state_path,
            "\n\nsw_volume: 90\n\nstate: play\n\n",
        )
        .unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.volume, 90);
        assert_eq!(loaded.state, Some(PlayerState::Play));
    }

    #[tokio::test]
    async fn test_elapsed_time_precision() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        let statefile = StateFile::new(state_path);

        let queue = Queue::new();
        let status = PlayerStatus {
            volume: 100,
            state: PlayerState::Play,
            current_song: None,
            next_song: None,
            elapsed: Some(Duration::from_millis(12345)),
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
            playlist_version: 1,
            playlist_length: 0,
            updating_db: None,
            error: None,
        };

        statefile.save(&status, &queue).await.unwrap();
        let loaded = statefile.load().unwrap().unwrap();
        assert!(loaded.elapsed_seconds.is_some());
        let elapsed = loaded.elapsed_seconds.unwrap();
        assert!((elapsed - 12.345).abs() < 0.001);
    }

    #[test]
    fn test_default_values() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state").to_str().unwrap().to_string();
        std::fs::write(&state_path, "").unwrap();

        let statefile = StateFile::new(state_path);
        let loaded = statefile.load().unwrap().unwrap();
        assert_eq!(loaded.volume, 0);
        assert_eq!(loaded.state, None);
        assert_eq!(loaded.current_position, None);
        assert_eq!(loaded.elapsed_seconds, None);
        assert!(!loaded.random);
        assert!(!loaded.repeat);
        assert_eq!(loaded.single, SingleMode::Off);
        assert_eq!(loaded.consume, ConsumeMode::Off);
        assert_eq!(loaded.crossfade, 0);
        assert_eq!(loaded.mixramp_db, 0.0);
        assert_eq!(loaded.mixramp_delay, 0.0);
        assert_eq!(loaded.playlist_paths.len(), 0);
    }
}
