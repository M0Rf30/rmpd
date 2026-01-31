//! Connection and server control commands
//!
//! This module handles commands related to server configuration, control,
//! and connection management.

use super::ResponseBuilder;

/// Return server configuration
///
/// TODO: Return actual configuration from AppState/Config
pub async fn handle_config_command() -> String {
    // Return configuration - minimal for now
    let mut resp = ResponseBuilder::new();
    resp.field("music_directory", "/var/lib/mpd/music");
    resp.ok()
}

/// Kill the server (graceful shutdown)
///
/// TODO: Implement graceful shutdown signaling
/// Should send shutdown signal to main server loop
pub async fn handle_kill_command() -> String {
    // Kill server (stub - should trigger graceful shutdown)
    ResponseBuilder::new().ok()
}
