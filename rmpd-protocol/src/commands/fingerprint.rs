use super::utils::ACK_ERROR_SYSTEM;
use super::ResponseBuilder;
use crate::state::AppState;
use rmpd_library::Fingerprinter;
use std::path::PathBuf;
use tracing::{debug, error};

/// Generate an audio fingerprint for a file
///
/// Uses Chromaprint to generate an AcoustID-compatible fingerprint.
/// The fingerprint can be used with the AcoustID service to identify music.
///
/// This operation is CPU-intensive and runs in a blocking task pool.
/// Only the first 120 seconds of audio are processed.
pub async fn handle_getfingerprint_command(state: &AppState, uri: &str) -> String {
    // Resolve the URI to an actual file path
    let path = match resolve_music_path(state, uri) {
        Ok(p) => p,
        Err(e) => {
            return ResponseBuilder::error(
                50,
                0,
                "getfingerprint",
                &format!("Failed to resolve URI: {e}"),
            );
        }
    };

    // Check if file exists
    if !path.exists() {
        return ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "getfingerprint", &format!("File not found: {uri}"));
    }

    debug!("Generating fingerprint for: {}", path.display());

    // Generate fingerprint in blocking task (CPU-intensive)
    match tokio::task::spawn_blocking(move || {
        let mut fingerprinter = Fingerprinter::new()?;
        fingerprinter.fingerprint_file(&path)
    })
    .await
    {
        Ok(Ok(fingerprint)) => {
            debug!("Fingerprint generated successfully");
            let mut resp = ResponseBuilder::new();
            resp.field("chromaprint", &fingerprint);
            resp.ok()
        }
        Ok(Err(e)) => {
            error!("Fingerprinting failed: {}", e);
            ResponseBuilder::error(
                50,
                0,
                "getfingerprint",
                &format!("Fingerprinting failed: {e}"),
            )
        }
        Err(_) => {
            error!("Fingerprinting task panicked");
            ResponseBuilder::error(ACK_ERROR_SYSTEM, 0, "getfingerprint", "Fingerprinting task panicked")
        }
    }
}

/// Resolve a URI to an absolute file path
fn resolve_music_path(state: &AppState, uri: &str) -> Result<PathBuf, String> {
    let music_dir = state
        .music_dir
        .as_ref()
        .ok_or_else(|| "Music directory not configured".to_string())?;

    // Remove leading slash if present
    let uri = uri.strip_prefix('/').unwrap_or(uri);

    // Security: Prevent path traversal attacks
    if uri.contains("..") {
        return Err("Path traversal not allowed".to_string());
    }

    let path = PathBuf::from(music_dir).join(uri);

    // Canonicalize to resolve symlinks and check bounds
    match path.canonicalize() {
        Ok(canonical) => {
            // Ensure the path is still within music_dir
            let music_dir_canonical = PathBuf::from(music_dir)
                .canonicalize()
                .map_err(|e| format!("Invalid music directory: {e}"))?;

            if canonical.starts_with(&music_dir_canonical) {
                Ok(canonical)
            } else {
                Err("Path outside music directory".to_string())
            }
        }
        Err(_) => {
            // File doesn't exist or can't be accessed, but return the path anyway
            // for better error messages
            Ok(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_music_path_traversal() {
        let state = AppState::with_paths("/tmp/db".to_string(), "/music".to_string());

        // Should reject path traversal
        let result = resolve_music_path(&state, "../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn test_resolve_music_path_normal() {
        let state = AppState::with_paths("/tmp/db".to_string(), "/tmp".to_string());

        // Normal path should work (even if file doesn't exist)
        let result = resolve_music_path(&state, "music/song.mp3");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_getfingerprint_no_music_dir() {
        let state = AppState::new();

        let response = handle_getfingerprint_command(&state, "song.mp3").await;

        // Should return error about missing music directory
        assert!(response.contains("ACK"));
        assert!(response.contains("Music directory"));
    }

    #[tokio::test]
    async fn test_getfingerprint_nonexistent_file() {
        let state = AppState::with_paths("/tmp/db".to_string(), "/tmp".to_string());

        let response = handle_getfingerprint_command(&state, "nonexistent.mp3").await;

        // Should return error about file not found
        assert!(response.contains("ACK"));
        assert!(response.contains("not found"));
    }
}
