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
    /// Multiple named passwords, each granting its own permission set
    /// (MPD's `password` config directive can repeat; see
    /// `rmpd_core::config::NetworkConfig::passwords`). The legacy `password`
    /// field above still means "one password granting every permission".
    pub passwords: Vec<rmpd_core::config::PasswordEntry>,
    /// Permissions granted to a connection before it authenticates, when no
    /// more specific rule (`local_permissions`/`host_permissions`) applies.
    /// `None` means "all, unless a password is configured" — see
    /// `crate::connection::resolve_initial_permissions`.
    pub default_permissions: Option<Vec<String>>,
    /// Permissions granted to unauthenticated connections over the local
    /// Unix domain socket. Takes precedence over `host_permissions` and
    /// `default_permissions`.
    pub local_permissions: Option<Vec<String>>,
    /// Per-peer-address permissions for unauthenticated remote (TCP)
    /// connections. Exact IP-literal match only (see `HostPermission`).
    pub host_permissions: Vec<rmpd_core::config::HostPermission>,
    /// Cap on an in-flight command-list batch's total byte size (MPD's
    /// `max_command_list_size`). Defaults to
    /// `crate::server::MAX_COMMAND_LIST_BYTES`.
    pub max_command_list_size: usize,
    /// Cap on a single response's byte size before the connection is
    /// closed (MPD's `max_output_buffer_size`). Defaults to
    /// `crate::server::DEFAULT_MAX_OUTPUT_BUFFER_BYTES`.
    pub max_output_buffer_size: usize,
    /// Cap on queue length (MPD's `max_playlist_length`), enforced with
    /// ACK 51. Defaults to `crate::commands::utils::DEFAULT_MAX_QUEUE_LEN`.
    pub max_playlist_length: u32,
    /// mDNS/Zeroconf instance-name template (`%h` expands to the
    /// hostname), matching MPD's `zeroconf_name`. Default `"rmpd@%h"`.
    pub zeroconf_name: String,
    /// Music-source registry built from `[[source]]` config blocks.
    pub sources: std::sync::Arc<rmpd_source::SourceRegistry>,
    /// Latest ICY "now playing" title for a remote stream (None when not
    /// streaming or no metadata has arrived). Injected into `currentsong`.
    pub stream_title: Arc<RwLock<Option<String>>>,
    /// Follow a symlink resolving inside `music_directory` when scanning.
    /// Mirrors `general.follow_inside_symlinks` from the config file.
    pub follow_inside_symlinks: bool,
    /// Follow a symlink resolving outside `music_directory` when scanning.
    /// Mirrors `general.follow_outside_symlinks` from the config file.
    pub follow_outside_symlinks: bool,
    /// Monotonic counter for library-scan job ids (MPD-style `updating_db`
    /// job numbers).
    job_counter: Arc<std::sync::atomic::AtomicU32>,
    /// Guards against concurrent music-source catalog syncs; a second
    /// `update` while one is running is a no-op (MPD serializes updates).
    source_sync_running: Arc<std::sync::atomic::AtomicBool>,
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
            passwords: Vec::new(),
            default_permissions: None,
            local_permissions: None,
            host_permissions: Vec::new(),
            max_command_list_size: crate::server::MAX_COMMAND_LIST_BYTES,
            max_output_buffer_size: crate::server::DEFAULT_MAX_OUTPUT_BUFFER_BYTES,
            max_playlist_length: crate::commands::utils::DEFAULT_MAX_QUEUE_LEN,
            zeroconf_name: "rmpd@%h".to_string(),
            stream_title: Arc::new(RwLock::new(None)),
            sources: std::sync::Arc::new(rmpd_source::SourceRegistry::from_config(&[])),
            follow_inside_symlinks: true,
            follow_outside_symlinks: true,
            job_counter: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            source_sync_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    /// Configure multiple named passwords (`[[network.passwords]]`), each
    /// granting its own permission set. Complements `set_password`, which
    /// stays for the legacy single "grants everything" password.
    pub fn set_passwords(&mut self, passwords: Vec<rmpd_core::config::PasswordEntry>) {
        self.passwords = passwords;
    }

    /// Configure the pre-auth permission rules (see
    /// `crate::connection::resolve_initial_permissions` for precedence).
    pub fn set_permission_rules(
        &mut self,
        default_permissions: Option<Vec<String>>,
        local_permissions: Option<Vec<String>>,
        host_permissions: Vec<rmpd_core::config::HostPermission>,
    ) {
        self.default_permissions = default_permissions;
        self.local_permissions = local_permissions;
        self.host_permissions = host_permissions;
    }

    /// Set the `network.max_command_list_size` cap (bytes).
    pub fn set_max_command_list_size(&mut self, n: usize) {
        self.max_command_list_size = n;
    }

    /// Set the `network.max_output_buffer_size` cap (bytes).
    pub fn set_max_output_buffer_size(&mut self, n: usize) {
        self.max_output_buffer_size = n;
    }

    /// Set the `general.max_playlist_length` queue cap (songs).
    pub fn set_max_playlist_length(&mut self, n: u32) {
        self.max_playlist_length = n;
    }

    /// Set the mDNS/Zeroconf instance-name template (`network.zeroconf_name`).
    pub fn set_zeroconf_name(&mut self, name: String) {
        self.zeroconf_name = name;
    }

    /// Set the music-source registry. Call at startup after building the
    /// registry from `[[source]]` config blocks.
    pub fn set_sources(&mut self, sources: std::sync::Arc<rmpd_source::SourceRegistry>) {
        self.sources = sources;
    }

    /// Configure the two independent MPD-style symlink-follow flags
    /// (`general.follow_inside_symlinks`/`general.follow_outside_symlinks`).
    pub fn set_symlink_policy(
        &mut self,
        follow_inside_symlinks: bool,
        follow_outside_symlinks: bool,
    ) {
        self.follow_inside_symlinks = follow_inside_symlinks;
        self.follow_outside_symlinks = follow_outside_symlinks;
    }

    pub fn advertise_mdns(&self, port: u16) {
        if let Some(discovery) = &self.discovery
            && let Err(e) = discovery.advertise(port, &self.zeroconf_name)
        {
            tracing::warn!("mDNS advertisement failed: {}", e);
        }
    }

    /// Spawn a background library scan of the configured music directory.
    ///
    /// Shared by the `update`/`rescan` commands and by auto-update on
    /// startup. `discard` forces re-reading every file's tags even if its
    /// mtime hasn't advanced (MPD's `rescan`; `update` passes `false`).
    ///
    /// Persists an incrementing job id to `status.updating_db` for the scan's
    /// duration (matching MPD's `status` response while a job is running)
    /// and returns it, or `None` if the database/music directory isn't
    /// configured. `Scanner::scan_directory` itself emits the
    /// `update`/`database` idle events.
    ///
    /// MPD semantics: if a scan is already running, `update` does not spawn
    /// a second one — it returns the in-progress job's id.
    pub async fn spawn_library_update(&self, discard: bool) -> Option<u32> {
        let (Some(db_path), Some(music_dir)) = (self.db_path.clone(), self.music_dir.clone())
        else {
            tracing::warn!("library update requested but database/music_dir not configured");
            return None;
        };
        let follow_inside_symlinks = self.follow_inside_symlinks;
        let follow_outside_symlinks = self.follow_outside_symlinks;
        let event_bus = self.event_bus.clone();
        let status = self.status.clone();

        let job_id = {
            let mut status_guard = self.status.write().await;
            if let Some(existing) = status_guard.updating_db {
                // A scan is already running; report its job id rather than
                // spawning a concurrent one (MPD's `update` behavior).
                return Some(existing);
            }
            let next = self
                .job_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            status_guard.updating_db = Some(next);
            next
        };

        tokio::spawn(async move {
            tracing::info!("starting library update (job {job_id})");
            let result = tokio::task::spawn_blocking(move || {
                let db = rmpd_library::Database::open(&db_path)?;
                let scanner = rmpd_library::Scanner::new(event_bus, false)
                    .with_symlink_policy(follow_inside_symlinks, follow_outside_symlinks)
                    .with_force_rescan(discard);
                scanner.scan_directory(&db, std::path::Path::new(&music_dir))
            })
            .await;

            match result {
                Ok(Ok(stats)) => tracing::info!(
                    "library scan complete: {} scanned, {} added, {} updated, {} removed, {} errors",
                    stats.scanned,
                    stats.added,
                    stats.updated,
                    stats.removed,
                    stats.errors
                ),
                Ok(Err(e)) => tracing::error!("library update failed: {}", e),
                Err(e) => tracing::error!("library update task panicked: {}", e),
            }

            status.write().await.updating_db = None;
        });

        Some(job_id)
    }

    /// Spawn a background source sync for every enabled music source.
    ///
    /// Each source is pinged first; on success the catalog is synced into the
    /// database via `rmpd_source::sync_source`. Sources that fail ping are
    /// skipped (cached rows are kept intact). Emits `DatabaseUpdateStarted` /
    /// `DatabaseUpdateFinished` idle events so waiting clients wake up.
    /// Does nothing when no sources are configured or the database is absent,
    /// and when a sync is already running (MPD serializes updates rather
    /// than racing concurrent `clear_source`+insert transactions).
    pub fn spawn_source_sync(&self) {
        let db_path = match self.db_path.clone() {
            Some(p) => p,
            None => return,
        };
        let sources = self.sources.clone();
        if sources.is_empty() {
            return;
        }
        if self
            .source_sync_running
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            tracing::debug!("source sync already in progress, skipping");
            return;
        }
        let event_bus = self.event_bus.clone();
        let running = self.source_sync_running.clone();

        tokio::spawn(async move {
            event_bus.emit(rmpd_core::event::Event::DatabaseUpdateStarted);
            for source in sources.iter() {
                let scheme = source.scheme().to_owned();
                let name = source.name().to_owned();
                match source.ping().await {
                    Ok(()) => {
                        tracing::info!("syncing music source '{}://{}'", scheme, name);
                        match rmpd_source::sync_source(source, &db_path).await {
                            Ok(count) => {
                                tracing::info!(
                                    "music source '{}://{}' synced {} songs",
                                    scheme,
                                    name,
                                    count
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "music source '{}://{}' sync error: {}",
                                    scheme,
                                    name,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "music source '{}://{}' ping failed, skipping sync: {}",
                            scheme,
                            name,
                            e
                        );
                    }
                }
            }
            event_bus.emit(rmpd_core::event::Event::DatabaseUpdateFinished);
            running.store(false, std::sync::atomic::Ordering::Release);
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
