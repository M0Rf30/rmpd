//! Connection and server control commands
//!
//! This module handles commands related to server configuration, control,
//! and connection management.

use super::{AppState, ResponseBuilder};
use crate::commands::utils::{ACK_ERROR_PASSWORD, ACK_ERROR_PERMISSION};
use crate::connection::ConnectionState;

/// Return server configuration
///
/// Returns server configuration information from AppState. MPD restricts
/// this command to clients connected over the local Unix socket
/// (`Client::IsLocal`); remote clients get an ACK.
pub async fn handle_config_command(state: &AppState, conn_state: &ConnectionState) -> String {
    if !conn_state.is_local {
        return ResponseBuilder::error(
            ACK_ERROR_PERMISSION,
            0,
            "config",
            "Command only permitted to local clients",
        );
    }

    let mut resp = ResponseBuilder::new();

    if let Some(music_dir) = &state.music_dir {
        resp.field("music_directory", music_dir);
    }

    if let Some(playlist_dir) = &state.playlist_dir {
        resp.field("playlist_directory", playlist_dir);
    }

    // rmpd has no PCRE support compiled in (matches MPD builds without
    // HAVE_PCRE): omit the field entirely rather than reporting "pcre: 0".
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

/// Handle the `password` command.
///
/// If no password is configured any value is accepted.
/// On success all permissions are granted; on failure an ACK error is returned.
pub async fn handle_password_command(
    state: &AppState,
    conn_state: &mut ConnectionState,
    password: &str,
) -> String {
    match &state.password {
        None => {
            // No password configured — any password is accepted, grant all permissions
            conn_state.grant_all_permissions();
            ResponseBuilder::new().ok()
        }
        Some(configured) => {
            if constant_time_eq(password.as_bytes(), configured.as_bytes()) {
                conn_state.grant_all_permissions();
                ResponseBuilder::new().ok()
            } else {
                ResponseBuilder::error(ACK_ERROR_PASSWORD, 0, "password", "incorrect password")
            }
        }
    }
}

/// Constant-time byte comparison to avoid leaking how many leading bytes of
/// a password match via short-circuiting `==` (timing side channel).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
