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
    BitrateChanged(Option<u32>), // Instantaneous bitrate in kbps (for VBR files)
    SongFinished,
    /// The engine advanced to the look-ahead (next) song in-thread — gaplessly
    /// or via crossfade — instead of stopping. The protocol promotes its fed
    /// "next" to current and feeds the following song.
    AdvancedToNext,
    /// A remote stream's ICY "now playing" title changed. Carries the new
    /// title (None clears it). Notifies the `player` subsystem so idle clients
    /// re-query `currentsong`.
    StreamTitleChanged(Option<String>),

    // Queue events
    QueueChanged,
    QueueOptionsChanged,

    // Stored playlist events
    /// The set or contents of on-disk stored playlists changed (save, rm,
    /// rename, playlistadd, playlistdelete, playlistclear, playlistmove,
    /// searchaddpl). Notifies the `stored_playlist` idle subsystem so clients
    /// re-query `listplaylists` / `listplaylistinfo`.
    StoredPlaylistChanged,

    // Database events
    DatabaseUpdateStarted,
    DatabaseUpdateProgress {
        scanned: u32,
        total: u32,
    },
    DatabaseUpdateFinished,

    // Output events
    OutputsChanged,

    // Filesystem watcher events
    FilesystemWatchStarted,
    FilesystemWatchStopped,
    SongAdded(Song),
    SongUpdated(Song),
    SongDeleted {
        path: String,
    },
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
            // Only notify idle for significant player events (state/song changes)
            // NOT for position/bitrate changes - those are too frequent and should be polled
            Event::PlayerStateChanged(_)
            | Event::SongChanged(_)
            | Event::SongFinished
            | Event::StreamTitleChanged(_) => &[Subsystem::Player],
            // Position and bitrate changes are internal - don't notify idle
            Event::PositionChanged(_) | Event::BitrateChanged(_) => &[],
            Event::VolumeChanged(_) => &[Subsystem::Mixer],
            Event::QueueChanged => &[Subsystem::Playlist],
            Event::QueueOptionsChanged => &[Subsystem::Options],
            Event::StoredPlaylistChanged => &[Subsystem::StoredPlaylist],
            Event::DatabaseUpdateStarted | Event::DatabaseUpdateProgress { .. } => {
                &[Subsystem::Update]
            }
            Event::DatabaseUpdateFinished => &[Subsystem::Database, Subsystem::Update],
            Event::SongAdded(_) | Event::SongUpdated(_) | Event::SongDeleted { .. } => {
                &[Subsystem::Database]
            }
            Event::OutputsChanged => &[Subsystem::Output],
            Event::FilesystemWatchStarted | Event::FilesystemWatchStopped => &[],
            _ => &[],
        }
    }
}

/// Central event bus
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(4096);
        Self { sender }
    }

    pub fn emit(&self, event: Event) {
        if let Err(e) = self.sender.send(event) {
            tracing::debug!("event dropped (no active subscribers): {}", e);
        }
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
