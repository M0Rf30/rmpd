use crate::discovery::DiscoveryService;
use rmpd_core::event::EventBus;
use rmpd_core::messaging::MessageBroker;
use rmpd_core::partition::PartitionManager;
use rmpd_core::queue::Queue;
use rmpd_core::state::PlayerStatus;
use rmpd_core::storage::MountRegistry;
use rmpd_player::PlaybackEngine;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{RwLock, broadcast};

/// Output device information
#[derive(Clone, Debug)]
pub struct OutputInfo {
    pub id: u32,
    pub name: String,
    pub plugin: String,
    pub enabled: bool,
    pub partition: Option<String>,
    pub config: Option<rmpd_core::config::OutputConfig>,
    pub attributes: std::collections::HashMap<String, String>,
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
    pub db_pool: Option<Arc<rmpd_library::DbPool>>,
    pub music_dir: Option<String>,
    pub playlist_dir: Option<String>,
    pub outputs: Arc<RwLock<Vec<OutputInfo>>>,
    pub start_time: Instant,
    pub message_broker: MessageBroker,
    pub discovery: Option<Arc<DiscoveryService>>,
    pub mount_registry: Arc<MountRegistry>,
    pub partition_manager: Option<Arc<PartitionManager>>,
    pub shutdown_tx: Option<broadcast::Sender<()>>,
    pub disable_actual_mount: bool,
    pub password: Option<String>,
    /// Latest ICY "now playing" title for a remote stream (None when not
    /// streaming or no metadata has arrived). Injected into `currentsong`.
    pub stream_title: Arc<RwLock<Option<String>>>,
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
    fn build(
        db_path: Option<String>,
        music_dir: Option<String>,
        playlist_dir: Option<String>,
    ) -> Self {
        let event_bus = EventBus::new();
        let status = Arc::new(RwLock::new(PlayerStatus::default()));
        let atomic_state = Arc::new(std::sync::atomic::AtomicU8::new(
            rmpd_core::state::PlayerState::Stop as u8,
        ));
        let engine = PlaybackEngine::new(event_bus.clone(), status.clone(), atomic_state.clone());

        let default_output = OutputInfo {
            id: 0,
            name: "Default Output".to_string(),
            plugin: "cpal".to_string(),
            enabled: true,
            partition: Some("default".to_string()),
            config: Some(rmpd_core::config::OutputConfig::cpal_default()),
            attributes: std::collections::HashMap::new(),
        };

        // Initialize discovery service (may fail if mDNS not available)
        let discovery = DiscoveryService::new().ok();
        if discovery.is_none() {
            tracing::warn!("failed to initialize network discovery service");
        }

        // Initialize mount registry
        let mount_registry = MountRegistry::new();

        // Initialize partition manager with default partition
        let partition_manager = PartitionManager::new();
        // Note: Creating the default partition is async, so we'll handle it during actual usage
        // For now, partition_manager exists but has no partitions until first command
        // Create a pooled database connection up front (schema is initialised
        // once here). Reused across commands so a chatty client doesn't pay the
        // cost of opening a fresh SQLite connection per request.
        let db_pool = db_path
            .as_ref()
            .and_then(|path| match rmpd_library::DbPool::new(path) {
                Ok(pool) => Some(pool),
                Err(e) => {
                    tracing::warn!("failed to create database connection pool: {e}");
                    None
                }
            });

        Self {
            queue: Arc::new(RwLock::new(Queue::new())),
            status,
            engine: Arc::new(RwLock::new(engine)),
            atomic_state,
            event_bus,
            db_path,
            db_pool,
            music_dir,
            playlist_dir,
            outputs: Arc::new(RwLock::new(vec![default_output])),
            start_time: Instant::now(),
            message_broker: MessageBroker::new(),
            discovery,
            mount_registry,
            partition_manager: Some(partition_manager),
            shutdown_tx: None,
            disable_actual_mount: std::env::var("RMPD_DISABLE_ACTUAL_MOUNT")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            password: None,
            stream_title: Arc::new(RwLock::new(None)),
        }
    }

    pub fn new() -> Self {
        Self::build(None, None, None)
    }

    pub fn with_paths(db_path: String, music_dir: String) -> Self {
        Self::build(Some(db_path), Some(music_dir), None)
    }

    pub fn with_all_paths(db_path: String, music_dir: String, playlist_dir: String) -> Self {
        Self::build(Some(db_path), Some(music_dir), Some(playlist_dir))
    }

    /// Set the shutdown sender for graceful shutdown support
    pub fn set_shutdown_sender(&mut self, tx: broadcast::Sender<()>) {
        self.shutdown_tx = Some(tx);
    }

    pub fn set_password(&mut self, password: Option<String>) {
        self.password = password;
    }

    pub fn advertise_mdns(&self, port: u16) {
        if let Some(ref discovery) = self.discovery
            && let Err(e) = discovery.advertise(port)
        {
            tracing::warn!("mDNS advertisement failed: {}", e);
        }
    }

    /// Spawn a background library scan of the configured music directory.
    ///
    /// Shared by the `update`/`rescan` commands and by auto-update on startup.
    /// Returns immediately; the scan runs on a blocking task and reports
    /// progress/results via the event bus and the tracing log. Does nothing if
    /// the database or music directory is not configured.
    pub fn spawn_library_update(&self) {
        let (Some(db_path), Some(music_dir)) = (self.db_path.clone(), self.music_dir.clone())
        else {
            tracing::warn!("library update requested but database/music_dir not configured");
            return;
        };
        let event_bus = self.event_bus.clone();

        tokio::task::spawn_blocking(move || {
            tracing::info!("starting library update");
            match rmpd_library::Database::open(&db_path) {
                Ok(db) => {
                    let scanner = rmpd_library::Scanner::new(event_bus.clone());
                    match scanner.scan_directory(&db, std::path::Path::new(&music_dir)) {
                        Ok(stats) => tracing::info!(
                            "library scan complete: {} scanned, {} added, {} updated, {} errors",
                            stats.scanned,
                            stats.added,
                            stats.updated,
                            stats.errors
                        ),
                        Err(e) => tracing::error!("library scan error: {}", e),
                    }
                }
                Err(e) => tracing::error!("failed to open database for update: {}", e),
            }
        });
    }

    pub async fn set_outputs_from_config(
        &self,
        outputs: &[rmpd_core::config::OutputConfig],
        default_name: &str,
    ) {
        let built: Vec<OutputInfo> = if outputs.is_empty() {
            vec![OutputInfo {
                id: 0,
                name: if default_name.is_empty() || default_name == "default" {
                    "Default Output".to_string()
                } else {
                    default_name.to_string()
                },
                plugin: "cpal".to_string(),
                enabled: true,
                partition: Some("default".to_string()),
                config: Some(rmpd_core::config::OutputConfig::cpal_default()),
                attributes: std::collections::HashMap::new(),
            }]
        } else {
            outputs
                .iter()
                .enumerate()
                .map(|(i, c)| OutputInfo {
                    id: i as u32,
                    name: c.name.clone(),
                    plugin: c.output_type.clone(),
                    enabled: c.enabled,
                    partition: Some("default".to_string()),
                    config: Some(c.clone()),
                    attributes: std::collections::HashMap::new(),
                })
                .collect()
        };
        *self.outputs.write().await = built;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
