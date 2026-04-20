use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::song::AudioFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerState {
    #[default]
    Stop,
    Play,
    Pause,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayGainMode {
    #[default]
    Off,
    Track,
    Album,
    Auto,
}

impl ReplayGainMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Track => "track",
            Self::Album => "album",
            Self::Auto => "auto",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "track" => Self::Track,
            "album" => Self::Album,
            "auto" => Self::Auto,
            _ => Self::Off,
        }
    }
}

impl std::fmt::Display for ReplayGainMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PlayerState {
    /// Convert from atomic u8 representation (Stop=0, Play=1, Pause=2)
    pub fn from_atomic(value: u8) -> Self {
        match value {
            0 => Self::Stop,
            1 => Self::Play,
            2 => Self::Pause,
            _ => Self::Stop,
        }
    }
}

impl std::fmt::Display for PlayerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stop => f.write_str("stop"),
            Self::Play => f.write_str("play"),
            Self::Pause => f.write_str("pause"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SingleMode {
    #[default]
    Off,
    On,
    Oneshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConsumeMode {
    #[default]
    Off,
    On,
    Oneshot,
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
    pub replay_gain_mode: ReplayGainMode,
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
            mixramp_db: 0.0,
            mixramp_delay: 0.0,
            playlist_version: 0,
            playlist_length: 0,
            updating_db: None,
            error: None,
            replay_gain_mode: ReplayGainMode::Off,
        }
    }
}

impl std::fmt::Display for PlayerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "state={} vol={} repeat={} random={} single={} consume={} playlist_length={}",
            self.state,
            self.volume,
            if self.repeat { "on" } else { "off" },
            if self.random { "on" } else { "off" },
            match self.single {
                SingleMode::Off => "off",
                SingleMode::On => "on",
                SingleMode::Oneshot => "oneshot",
            },
            match self.consume {
                ConsumeMode::Off => "off",
                ConsumeMode::On => "on",
                ConsumeMode::Oneshot => "oneshot",
            },
            self.playlist_length
        )
    }
}
