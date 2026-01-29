use anyhow::Result;
use rmpd_core::config::Config;
use rmpd_protocol::{AppState, MpdServer};

pub async fn run(bind_address: String, config: Config) -> Result<()> {
    // Create application state with database and music directory paths
    let db_path = config.general.db_file.to_string();
    let music_dir = config.general.music_directory.to_string();

    let state = AppState::with_paths(db_path, music_dir);

    // Create and run server
    let server = MpdServer::with_state(bind_address, state);
    server.run().await?;
    Ok(())
}
