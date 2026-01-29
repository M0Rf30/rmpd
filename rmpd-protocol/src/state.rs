use rmpd_core::event::EventBus;
use rmpd_core::queue::Queue;
use rmpd_core::state::PlayerStatus;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub queue: Arc<RwLock<Queue>>,
    pub status: Arc<RwLock<PlayerStatus>>,
    pub event_bus: EventBus,
    pub db_path: Option<String>,
    pub music_dir: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            event_bus: EventBus::new(),
            db_path: None,
            music_dir: None,
        }
    }

    pub fn with_paths(db_path: String, music_dir: String) -> Self {
        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            event_bus: EventBus::new(),
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
