use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::song::AudioFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerState {
    Stop,
    Play,
    Pause,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::Stop
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SingleMode {
    Off,
    On,
    Oneshot,
}

impl Default for SingleMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsumeMode {
    Off,
    On,
    Oneshot,
}

impl Default for ConsumeMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct QueuePosition {
    pub position: u32,
    pub id: u32,
}

/// Complete player status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub state: PlayerState,
    pub volume: u8,
    pub repeat: bool,
    pub random: bool,
    pub single: SingleMode,
    pub consume: ConsumeMode,
    pub current_song: Option<QueuePosition>,
    pub next_song: Option<QueuePosition>,
    pub elapsed: Option<Duration>,
    pub duration: Option<Duration>,
    pub bitrate: Option<u32>,
    pub audio_format: Option<AudioFormat>,
    pub crossfade: u32,
    pub mixramp_db: f32,
    pub mixramp_delay: f32,
    pub playlist_version: u32,
    pub playlist_length: u32,
    pub updating_db: Option<u32>,
    pub error: Option<String>,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self {
            state: PlayerState::Stop,
            volume: 100,
            repeat: false,
            random: false,
            single: SingleMode::Off,
            consume: ConsumeMode::Off,
            current_song: None,
            next_song: None,
            elapsed: None,
            duration: None,
            bitrate: None,
            audio_format: None,
            crossfade: 0,
            mixramp_db: -17.0,
            mixramp_delay: 0.0,
            playlist_version: 0,
            playlist_length: 0,
            updating_db: None,
            error: None,
        }
    }
}
