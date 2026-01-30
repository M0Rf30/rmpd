use anyhow::Result;
use rmpd_core::config::Config;
use rmpd_protocol::{AppState, MpdServer, StateFile};
use tokio::signal;
use tracing::{info, warn, error};

pub async fn run(bind_address: String, config: Config) -> Result<()> {
    // Create application state with database and music directory paths
    let db_path = config.general.db_file.to_string();
    let music_dir = config.general.music_directory.to_string();
    let state_file_path = config.general.state_file.to_string();

    let state = AppState::with_paths(db_path.clone(), music_dir.clone());

    // Load state from file if it exists
    let state_file = StateFile::new(state_file_path.clone());
    if let Ok(Some(saved_state)) = state_file.load() {
        info!("Restoring state from file");
        restore_state(&state, saved_state, &db_path, &music_dir).await;
    }

    // Clone state for shutdown handler
    let shutdown_state = state.clone();
    let shutdown_state_file_path = state_file_path.clone();

    // Spawn task to handle shutdown signals
    tokio::spawn(async move {
        match signal::ctrl_c().await {
            Ok(()) => {
                info!("Received SIGINT, saving state...");
                save_state_on_shutdown(&shutdown_state, &shutdown_state_file_path).await;
            }
            Err(err) => {
                error!("Unable to listen for shutdown signal: {}", err);
            }
        }
    });

    // Create and run server
    let server = MpdServer::with_state(bind_address, state.clone());

    // Run server and handle result
    let server_result = server.run().await;

    // Save state on clean shutdown
    info!("Server stopped, saving state...");
    save_state_on_shutdown(&state, &state_file_path).await;

    server_result?;
    Ok(())
}

async fn restore_state(state: &AppState, saved_state: rmpd_protocol::statefile::SavedState, db_path: &str, _music_dir: &str) {
    // Restore playback options
    {
        let mut status = state.status.write().await;
        status.volume = saved_state.volume;
        status.random = saved_state.random;
        status.repeat = saved_state.repeat;
        status.single = saved_state.single;
        status.consume = saved_state.consume;
        status.crossfade = saved_state.crossfade;
        status.mixramp_db = saved_state.mixramp_db;
        status.mixramp_delay = saved_state.mixramp_delay;
    }

    // Restore playlist
    if !saved_state.playlist_paths.is_empty() {
        info!("Restoring playlist with {} songs", saved_state.playlist_paths.len());

        if let Ok(db) = rmpd_library::Database::open(db_path) {
            let mut queue = state.queue.write().await;

            for path in &saved_state.playlist_paths {
                // Try to find song in database
                if let Ok(Some(song)) = db.get_song_by_path(path) {
                    queue.add(song);
                } else {
                    warn!("Song not found in database: {}", path);
                }
            }

            let playlist_len = queue.len() as u32;
            drop(queue);

            // Update playlist length in status
            let mut status = state.status.write().await;
            status.playlist_length = playlist_len;
        }
    }

    // Restore current song position in status (but don't auto-start playback)
    // MPD doesn't auto-resume playback on restart - user must manually play
    if let Some(position) = saved_state.current_position {
        info!("Setting current position to {}", position);
        let queue = state.queue.read().await;
        if let Some(item) = queue.get(position) {
            let song_id = item.id;
            let mut status = state.status.write().await;
            status.current_song = Some(rmpd_core::state::QueuePosition {
                position,
                id: song_id,
            });
            // Note: Playback state is intentionally set to Stop
            // User must manually start playback after daemon restart
        }
    }

    info!("State restoration complete");
}

async fn save_state_on_shutdown(state: &AppState, state_file_path: &str) {
    let status = state.status.read().await;
    let queue = state.queue.read().await;

    let state_file = StateFile::new(state_file_path.to_string());
    if let Err(e) = state_file.save(&status, &queue).await {
        error!("Failed to save state: {}", e);
    }
}
