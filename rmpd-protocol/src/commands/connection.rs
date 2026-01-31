//! Connection and server control commands
//!
//! This module handles commands related to server configuration, control,
//! and connection management.

use super::{AppState, ResponseBuilder};

/// Return server configuration
///
/// Returns server configuration information from AppState.
pub async fn handle_config_command(state: &AppState) -> String {
    let mut resp = ResponseBuilder::new();

    if let Some(music_dir) = &state.music_dir {
        resp.field("music_directory", music_dir);
    }

    if let Some(db_path) = &state.db_path {
        resp.field("db_file", db_path);
    }

    resp.ok()
}

/// Kill the server (graceful shutdown)
///
/// Sends a shutdown signal to the main server loop, triggering graceful shutdown.
pub async fn handle_kill_command(state: &AppState) -> String {
    if let Some(shutdown_tx) = &state.shutdown_tx {
        // Send shutdown signal (ignore error if no receivers)
        let _ = shutdown_tx.send(());
    }
    ResponseBuilder::new().ok()
}
