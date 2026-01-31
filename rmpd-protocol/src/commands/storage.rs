//! Storage and mount commands
//!
//! MPD supports mounting remote storage locations and discovering network neighbors.
//! This module handles storage-related commands.
//!
//! IMPLEMENTATION STATUS:
//! All commands in this module are stubs. Full implementation requires:
//! - FUSE or system mount integration for virtual filesystems
//! - mDNS/Avahi for network discovery
//! - Platform-specific code for Linux/macOS/Windows
//! - Protocol support for NFS, SMB, WebDAV, UPnP, AirPlay

use super::ResponseBuilder;

/// Mount a storage location
///
/// IMPLEMENTATION NOTE:
/// Full storage mounting requires:
/// 1. Filesystem integration (fuse-rs or system mount commands)
/// 2. Protocol support: NFS (nfs crate), SMB (smbclient), WebDAV (reqwest)
/// 3. Mount point management in music directory
/// 4. Permission and security handling
/// 5. Platform-specific code (Linux/macOS/Windows differ)
///
/// Returns empty OK for compatibility. Commands accepted but no actual mounting occurs.
pub async fn handle_mount_command(_path: &str, _uri: &str) -> String {
    // Stub: Accepts mount commands but doesn't perform mounting
    ResponseBuilder::new().ok()
}

/// Unmount a storage location
///
/// IMPLEMENTATION NOTE:
/// Unmounting requires tracking active mounts and calling umount/fusermount.
///
/// Returns empty OK for compatibility.
pub async fn handle_unmount_command(_path: &str) -> String {
    // Stub: Accepts unmount commands but doesn't perform unmounting
    ResponseBuilder::new().ok()
}

/// List all mounted storage locations
///
/// IMPLEMENTATION NOTE:
/// Returns empty list. Would need to track mounts in AppState and
/// return them in MPD format: mount: uri\nstorage: path\n
pub async fn handle_listmounts_command() -> String {
    // Return empty list (no mounts tracked)
    ResponseBuilder::new().ok()
}

/// List network neighbors for storage discovery
///
/// IMPLEMENTATION NOTE:
/// Network discovery requires:
/// 1. mDNS/DNS-SD support (zeroconf/mdns-sd crate)
/// 2. UPnP discovery (rupnp crate)
/// 3. Avahi integration on Linux (avahi-sys)
/// 4. Bonjour on macOS (system APIs)
/// 5. Protocol parsing (MPD, AirPlay, DLNA, etc.)
///
/// For production implementation:
/// - Integrate mdns-sd crate for cross-platform mDNS
/// - Scan for _mpd._tcp, _airplay._tcp, _upnp._tcp services
/// - Return discovered neighbors with: neighbor: protocol://address\nname: friendly_name\n
pub async fn handle_listneighbors_command() -> String {
    // Return empty list (no discovery implemented)
    ResponseBuilder::new().ok()
}
