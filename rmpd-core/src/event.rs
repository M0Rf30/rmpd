use crate::song::Song;
use crate::state::PlayerState;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::broadcast;

/// Events that can be emitted by any component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    // Player events
    PlayerStateChanged(PlayerState),
    SongChanged(Option<Song>),
    PositionChanged(Duration),
    VolumeChanged(u8),
    SongFinished,

    // Queue events
    QueueChanged,
    QueueOptionsChanged,

    // Database events
    DatabaseUpdateStarted,
    DatabaseUpdateProgress { scanned: u32, total: u32 },
    DatabaseUpdateFinished,

    // Output events
    OutputsChanged,

    // Connection events
    ClientConnected(u64),
    ClientDisconnected(u64),

    // Plugin events
    PluginLoaded(String),
    PluginUnloaded(String),

    // Partition events
    PartitionChanged(String),
}

/// Maps to MPD's idle subsystems
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Subsystem {
    Database,
    Update,
    StoredPlaylist,
    Playlist,
    Player,
    Mixer,
    Output,
    Options,
    Partition,
    Sticker,
    Subscription,
    Message,
    Neighbor,
    Mount,
}

impl Event {
    pub fn subsystems(&self) -> &'static [Subsystem] {
        match self {
            Event::PlayerStateChanged(_) | Event::SongChanged(_) |
            Event::PositionChanged(_) => &[Subsystem::Player],
            Event::VolumeChanged(_) => &[Subsystem::Mixer],
            Event::QueueChanged => &[Subsystem::Playlist],
            Event::QueueOptionsChanged => &[Subsystem::Options],
            Event::DatabaseUpdateStarted | Event::DatabaseUpdateProgress { .. } =>
                &[Subsystem::Update],
            Event::DatabaseUpdateFinished => &[Subsystem::Database, Subsystem::Update],
            Event::OutputsChanged => &[Subsystem::Output],
            Event::PartitionChanged(_) => &[Subsystem::Partition],
            _ => &[],
        }
    }
}

/// Central event bus
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }

    pub fn emit(&self, event: Event) {
        // Ignore errors - means no subscribers
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
