//! Storage and mount commands
//!
//! MPD supports mounting remote storage locations and discovering network neighbors.
//! This module handles storage-related commands.

use super::ResponseBuilder;

/// Mount a storage location
///
/// TODO: Implement virtual filesystem mounting for remote storage
/// Would require integration with network protocols (SMB, NFS, etc.)
pub async fn handle_mount_command(_path: &str, _uri: &str) -> String {
    // Mount storage (not implemented - would require virtual FS)
    ResponseBuilder::new().ok()
}

/// Unmount a storage location
///
/// TODO: Implement unmounting with cleanup
pub async fn handle_unmount_command(_path: &str) -> String {
    // Unmount storage (not implemented)
    ResponseBuilder::new().ok()
}

/// List all mounted storage locations
///
/// Currently returns empty list as mounting is not implemented
pub async fn handle_listmounts_command() -> String {
    // List mounts - return empty
    ResponseBuilder::new().ok()
}

/// List network neighbors for storage discovery
///
/// TODO: Implement network discovery (Avahi/Bonjour, UPnP, etc.)
pub async fn handle_listneighbors_command() -> String {
    // List network neighbors - return empty
    ResponseBuilder::new().ok()
}
