//! Connection and server control commands
//!
//! This module handles commands related to server configuration, control,
//! and connection management.

use super::{AppState, ResponseBuilder};
use crate::commands::utils::{ACK_ERROR_PASSWORD, ACK_ERROR_PERMISSION};
use crate::connection::{ConnectionState, PERMISSION_ALL, permission_bits_from_names};

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
/// If no password is configured at all (neither the legacy single
/// `[network] password` nor any `[[network.passwords]]` entry) any value is
/// accepted and all permissions are granted, matching rmpd's historical
/// behaviour. Otherwise every configured password is compared in constant
/// time — so a timing side channel can't reveal which one (or whether any)
/// matched — and, on a match, the connection's permission set is REPLACED
/// (not OR'd in) with that password's granted set: the legacy password
/// still grants everything; each `PasswordEntry` grants only its own list.
pub async fn handle_password_command(
    state: &AppState,
    conn_state: &mut ConnectionState,
    password: &str,
) -> String {
    if state.password.is_none() && state.passwords.is_empty() {
        // No password configured — any password is accepted, grant all permissions.
        conn_state.grant_all_permissions();
        return ResponseBuilder::new().ok();
    }

    let candidate = password.as_bytes();
    let mut matched: Option<u8> = None;

    if let Some(legacy) = &state.password
        && constant_time_eq(candidate, legacy.as_bytes())
    {
        matched = Some(PERMISSION_ALL);
    }

    // Compare against every configured entry regardless of an earlier
    // match — `constant_time_eq` always runs for each one — so the loop
    // itself doesn't leak which password (if any) matched via timing.
    for entry in &state.passwords {
        if constant_time_eq(candidate, entry.password.as_bytes()) && matched.is_none() {
            matched = Some(permission_bits_from_names(&entry.permissions));
        }
    }

    match matched {
        Some(perms) => {
            conn_state.set_permissions(perms);
            ResponseBuilder::new().ok()
        }
        None => ResponseBuilder::error(ACK_ERROR_PASSWORD, 0, "password", "incorrect password"),
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
