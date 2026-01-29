use rmpd_core::event::EventBus;
use rmpd_core::queue::Queue;
use rmpd_core::state::PlayerStatus;
use rmpd_player::PlaybackEngine;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub queue: Arc<RwLock<Queue>>,
    pub status: Arc<RwLock<PlayerStatus>>,
    pub engine: Arc<RwLock<PlaybackEngine>>,
    pub event_bus: EventBus,
    pub db_path: Option<String>,
    pub music_dir: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        let event_bus = EventBus::new();
        let engine = PlaybackEngine::new(event_bus.clone());

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            engine: Arc::new(RwLock::new(engine)),
            event_bus,
            db_path: None,
            music_dir: None,
        }
    }

    pub fn with_paths(db_path: String, music_dir: String) -> Self {
        let event_bus = EventBus::new();
        let engine = PlaybackEngine::new(event_bus.clone());

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            engine: Arc::new(RwLock::new(engine)),
            event_bus,
            db_path: Some(db_path),
            music_dir: Some(music_dir),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
