use rmpd_core::event::EventBus;
use rmpd_core::messaging::MessageBroker;
use rmpd_core::queue::Queue;
use rmpd_core::state::PlayerStatus;
use rmpd_player::PlaybackEngine;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};

/// Output device information
#[derive(Clone, Debug)]
pub struct OutputInfo {
    pub id: u32,
    pub name: String,
    pub plugin: String,
    pub enabled: bool,
}

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub queue: Arc<RwLock<Queue>>,
    pub status: Arc<RwLock<PlayerStatus>>,
    pub engine: Arc<RwLock<PlaybackEngine>>,
    pub atomic_state: Arc<std::sync::atomic::AtomicU8>, // Lock-free state access
    pub event_bus: EventBus,
    pub db_path: Option<String>,
    pub music_dir: Option<String>,
    pub outputs: Arc<RwLock<Vec<OutputInfo>>>,
    pub start_time: Instant,
    pub message_broker: MessageBroker,
    pub shutdown_tx: Option<broadcast::Sender<()>>,
}

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("event_bus", &self.event_bus)
            .field("db_path", &self.db_path)
            .field("music_dir", &self.music_dir)
            .field("start_time", &self.start_time)
            .finish_non_exhaustive()
    }
}

impl AppState {
    fn build(db_path: Option<String>, music_dir: Option<String>) -> Self {
        let event_bus = EventBus::new();
        let status = Arc::new(RwLock::new(PlayerStatus::default()));
        let atomic_state = Arc::new(std::sync::atomic::AtomicU8::new(
            rmpd_core::state::PlayerState::Stop as u8,
        ));
        let engine = PlaybackEngine::new(event_bus.clone(), status.clone(), atomic_state.clone());

        // Create default output
        let default_output = OutputInfo {
            id: 0,
            name: "Default Output".to_string(),
            plugin: "cpal".to_string(),
            enabled: true,
        };

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status,
            engine: Arc::new(RwLock::new(engine)),
            atomic_state,
            event_bus,
            db_path,
            music_dir,
            outputs: Arc::new(RwLock::new(vec![default_output])),
            start_time: Instant::now(),
            message_broker: MessageBroker::new(),
            shutdown_tx: None,
        }
    }

    pub fn new() -> Self {
        Self::build(None, None)
    }

    pub fn with_paths(db_path: String, music_dir: String) -> Self {
        Self::build(Some(db_path), Some(music_dir))
    }

    /// Set the shutdown sender for graceful shutdown support
    pub fn set_shutdown_sender(&mut self, tx: broadcast::Sender<()>) {
        self.shutdown_tx = Some(tx);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
