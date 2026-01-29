use rmpd_core::event::EventBus;
use rmpd_core::queue::Queue;
use rmpd_core::state::PlayerStatus;
use rmpd_player::PlaybackEngine;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

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
    pub event_bus: EventBus,
    pub db_path: Option<String>,
    pub music_dir: Option<String>,
    pub outputs: Arc<RwLock<Vec<OutputInfo>>>,
    pub start_time: Instant,
}

impl AppState {
    pub fn new() -> Self {
        let event_bus = EventBus::new();
        let engine = PlaybackEngine::new(event_bus.clone());

        // Create default output
        let default_output = OutputInfo {
            id: 0,
            name: "Default Output".to_string(),
            plugin: "cpal".to_string(),
            enabled: true,
        };

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            engine: Arc::new(RwLock::new(engine)),
            event_bus,
            db_path: None,
            music_dir: None,
            outputs: Arc::new(RwLock::new(vec![default_output])),
            start_time: Instant::now(),
        }
    }

    pub fn with_paths(db_path: String, music_dir: String) -> Self {
        let event_bus = EventBus::new();
        let engine = PlaybackEngine::new(event_bus.clone());

        // Create default output
        let default_output = OutputInfo {
            id: 0,
            name: "Default Output".to_string(),
            plugin: "cpal".to_string(),
            enabled: true,
        };

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status: Arc::new(RwLock::new(PlayerStatus::default())),
            engine: Arc::new(RwLock::new(engine)),
            event_bus,
            db_path: Some(db_path),
            music_dir: Some(music_dir),
            outputs: Arc::new(RwLock::new(vec![default_output])),
            start_time: Instant::now(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
