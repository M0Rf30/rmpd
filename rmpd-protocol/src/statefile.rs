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
        content.push_str(&format!("state: {}\n", state_str));

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
        content.push_str(&format!("single: {}\n", single_val));

        let consume_val = match status.consume {
            rmpd_core::state::ConsumeMode::Off => 0,
            rmpd_core::state::ConsumeMode::On => 1,
            rmpd_core::state::ConsumeMode::Oneshot => 2,
        };
        content.push_str(&format!("consume: {}\n", consume_val));

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
