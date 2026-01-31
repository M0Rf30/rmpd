//! Storage and mount commands
//!
//! MPD supports mounting remote storage locations and discovering network neighbors.
//! This module handles storage-related commands.
//!
//! IMPLEMENTATION STATUS:
//! - listneighbors: ✅ Fully implemented with mDNS discovery
//! - mount/unmount/listmounts: ✅ Tier 1 (tracking) + Tier 2 (actual mounting) implemented

use super::ResponseBuilder;
use crate::state::AppState;
use rmpd_core::storage::platform::get_default_backend;
use std::path::PathBuf;

/// Mount a storage location
///
/// Tier 2 Implementation: Performs actual filesystem mounting using platform backends.
/// URI format: protocol://address/path (e.g., nfs://192.168.1.100/music)
///
/// Supports NFS, SMB/CIFS, and WebDAV (with davfs2) on Linux.
/// Requires appropriate permissions (may need sudo/polkit configuration).
///
/// Set RMPD_DISABLE_ACTUAL_MOUNT=1 environment variable to disable actual mounting
/// and only track mounts in registry (Tier 1 mode).
pub async fn handle_mount_command(state: &AppState, path: &str, uri: &str) -> String {
    // Validate path (no ../, no absolute paths)
    if path.contains("..") || path.starts_with('/') {
        return ResponseBuilder::error(
            50,
            0,
            "mount",
            "Invalid path: no absolute paths or path traversal allowed",
        );
    }

    // Check if music directory is configured
    let music_dir = match &state.music_dir {
        Some(dir) => dir,
        None => {
            return ResponseBuilder::error(
                50,
                0,
                "mount",
                "Music directory not configured",
            );
        }
    };

    // Create full mountpoint path
    let mountpoint = PathBuf::from(music_dir).join(path);

    // Check if actual mounting is disabled
    let disable_actual_mount = std::env::var("RMPD_DISABLE_ACTUAL_MOUNT")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    if !disable_actual_mount {
        // Tier 2: Perform actual mounting
        tracing::info!("Mounting {} to {}", uri, mountpoint.display());

        // Create mountpoint directory if it doesn't exist
        if let Err(e) = tokio::fs::create_dir_all(&mountpoint).await {
            return ResponseBuilder::error(
                50,
                0,
                "mount",
                &format!("Failed to create mountpoint: {e}"),
            );
        }

        // Perform mount in blocking task (system calls)
        let uri_clone = uri.to_string();
        let mountpoint_clone = mountpoint.clone();

        match tokio::task::spawn_blocking(move || {
            let backend = get_default_backend();
            backend.mount(&uri_clone, &mountpoint_clone, &[])
        })
        .await
        {
            Ok(Ok(_)) => {
                tracing::info!("Successfully mounted {} to {}", uri, mountpoint.display());

                // Register as mounted in registry
                if let Err(e) = state
                    .mount_registry
                    .register_mounted(path.to_string(), uri.to_string())
                    .await
                {
                    tracing::error!("Failed to register mount: {}", e);
                    return ResponseBuilder::error(50, 0, "mount", &format!("Mount succeeded but registration failed: {e}"));
                }

                ResponseBuilder::new().ok()
            }
            Ok(Err(e)) => {
                tracing::error!("Mount failed: {}", e);
                ResponseBuilder::error(50, 0, "mount", &format!("Mount failed: {e}"))
            }
            Err(_) => {
                tracing::error!("Mount task panicked");
                ResponseBuilder::error(50, 0, "mount", "Mount task panicked")
            }
        }
    } else {
        // Tier 1: Only register mount without actual mounting
        tracing::info!("Registering mount (actual mounting disabled): {} -> {}", path, uri);

        match state
            .mount_registry
            .register(path.to_string(), uri.to_string())
            .await
        {
            Ok(_) => ResponseBuilder::new().ok(),
            Err(e) => ResponseBuilder::error(50, 0, "mount", &format!("Mount registration failed: {e}")),
        }
    }
}

/// Unmount a storage location
///
/// Tier 2 Implementation: Performs actual filesystem unmounting using platform backends.
///
/// Set RMPD_DISABLE_ACTUAL_MOUNT=1 environment variable to disable actual unmounting
/// and only remove from registry (Tier 1 mode).
pub async fn handle_unmount_command(state: &AppState, path: &str) -> String {
    // Check if music directory is configured
    let music_dir = match &state.music_dir {
        Some(dir) => dir,
        None => {
            return ResponseBuilder::error(
                50,
                0,
                "unmount",
                "Music directory not configured",
            );
        }
    };

    // Create full mountpoint path
    let mountpoint = PathBuf::from(music_dir).join(path);

    // Check if actual mounting is disabled
    let disable_actual_mount = std::env::var("RMPD_DISABLE_ACTUAL_MOUNT")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);

    if !disable_actual_mount {
        // Tier 2: Perform actual unmounting
        tracing::info!("Unmounting {}", mountpoint.display());

        // Perform unmount in blocking task (system calls)
        let mountpoint_clone = mountpoint.clone();

        match tokio::task::spawn_blocking(move || {
            let backend = get_default_backend();
            backend.unmount(&mountpoint_clone)
        })
        .await
        {
            Ok(Ok(_)) => {
                tracing::info!("Successfully unmounted {}", mountpoint.display());

                // Remove from registry
                if let Err(e) = state.mount_registry.unmount(path).await {
                    tracing::error!("Failed to unregister mount: {}", e);
                }

                ResponseBuilder::new().ok()
            }
            Ok(Err(e)) => {
                tracing::error!("Unmount failed: {}", e);
                // Still try to remove from registry
                let _ = state.mount_registry.unmount(path).await;
                ResponseBuilder::error(50, 0, "unmount", &format!("Unmount failed: {e}"))
            }
            Err(_) => {
                tracing::error!("Unmount task panicked");
                ResponseBuilder::error(50, 0, "unmount", "Unmount task panicked")
            }
        }
    } else {
        // Tier 1: Only remove from registry
        tracing::info!("Unregistering mount (actual unmounting disabled): {}", path);

        match state.mount_registry.unmount(path).await {
            Ok(_) => ResponseBuilder::new().ok(),
            Err(e) => ResponseBuilder::error(50, 0, "unmount", &format!("Unmount failed: {e}")),
        }
    }
}

/// List all mounted storage locations
///
/// Returns registered mounts in MPD format:
/// - mount: uri
/// - storage: path
pub async fn handle_listmounts_command(state: &AppState) -> String {
    let mounts = state.mount_registry.list().await;

    let mut resp = ResponseBuilder::new();
    for mount in mounts {
        resp.field("mount", &mount.uri);
        resp.field("storage", &mount.path);
    }

    resp.ok()
}

/// List network neighbors for storage discovery
///
/// Scans the local network for MPD servers, SMB shares, NFS servers, and HTTP/WebDAV services.
/// Uses mDNS/DNS-SD for discovery. Results are cached for 5 minutes.
///
/// Returns:
/// - neighbor: protocol://address (e.g., mpd://192.168.1.100:6600)
/// - name: friendly_name (service name from mDNS)
///
/// If discovery service is unavailable, returns empty list.
pub async fn handle_listneighbors_command(state: &AppState) -> String {
    // Check if discovery service is available
    let discovery = match &state.discovery {
        Some(d) => d,
        None => {
            // Discovery not available (mDNS initialization failed)
            return ResponseBuilder::new().ok();
        }
    };

    // Scan for network services
    match discovery.scan_services().await {
        Ok(neighbors) => {
            let mut resp = ResponseBuilder::new();

            for neighbor in neighbors {
                // Format: neighbor: protocol://address
                resp.field("neighbor", &format!("{}://{}", neighbor.protocol, neighbor.address));
                resp.field("name", &neighbor.name);
            }

            resp.ok()
        }
        Err(e) => {
            // Log error but return empty list (graceful degradation)
            tracing::error!("Network discovery failed: {}", e);
            ResponseBuilder::new().ok()
        }
    }
}
