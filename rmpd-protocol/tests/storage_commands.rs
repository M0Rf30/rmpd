//! Storage command tests
//!
//! These tests verify mount/unmount/listmounts commands.
//!
//! **IMPORTANT**: These tests disable actual mounting automatically to avoid
//! requiring root privileges. They test the mount tracking/registry functionality.

use rmpd_protocol::AppState;
use rmpd_protocol::commands::storage;

fn test_state() -> AppState {
    let mut state = AppState::with_paths("/tmp/test_db".to_string(), "/tmp/test_music".to_string());
    state.disable_actual_mount = true;
    state
}

#[tokio::test]
async fn test_mount_command() {
    let state = test_state();

    let response =
        storage::handle_mount_command(&state, "remote/nas", "nfs://192.168.1.100/music").await;

    assert_eq!(response, "OK\n");

    // Verify mount was registered
    let mounts = state.mount_registry.list().await;
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0].path, "remote/nas");
    assert_eq!(mounts[0].uri, "nfs://192.168.1.100/music");
    assert_eq!(mounts[0].protocol, "nfs");
}

#[tokio::test]
async fn test_mount_duplicate() {
    let state = test_state();

    // First mount should succeed
    let response1 =
        storage::handle_mount_command(&state, "remote/nas", "nfs://192.168.1.100/music").await;
    assert_eq!(response1, "OK\n");

    // Second mount to same path should fail
    let response2 =
        storage::handle_mount_command(&state, "remote/nas", "nfs://192.168.1.200/music").await;
    assert!(response2.contains("ACK"));
    assert!(response2.contains("already exists"));
}

#[tokio::test]
async fn test_mount_path_validation() {
    let state = test_state();

    // Absolute path should be rejected
    let response1 =
        storage::handle_mount_command(&state, "/etc/passwd", "nfs://server/share").await;
    assert!(response1.contains("ACK"));
    assert!(response1.contains("Invalid path"));

    // Path traversal should be rejected
    let response2 =
        storage::handle_mount_command(&state, "../etc/passwd", "nfs://server/share").await;
    assert!(response2.contains("ACK"));
    assert!(response2.contains("Invalid path"));
}

#[tokio::test]
async fn test_unmount_command() {
    let state = test_state();

    // Mount first
    storage::handle_mount_command(&state, "remote/nas", "nfs://192.168.1.100/music").await;

    // Unmount
    let response = storage::handle_unmount_command(&state, "remote/nas").await;
    assert_eq!(response, "OK\n");

    // Verify mount was removed
    let mounts = state.mount_registry.list().await;
    assert_eq!(mounts.len(), 0);
}

#[tokio::test]
async fn test_unmount_nonexistent() {
    let state = test_state();

    let response = storage::handle_unmount_command(&state, "nonexistent").await;
    assert!(response.contains("ACK"));
    // In Tier 1 mode, unmounting unregistered mount returns error
    assert!(response.contains("not found") || response.contains("failed"));
}

#[tokio::test]
async fn test_listmounts_command() {
    let state = test_state();

    // Empty list initially
    let response1 = storage::handle_listmounts_command(&state).await;
    assert_eq!(response1, "OK\n");

    // Add some mounts
    storage::handle_mount_command(&state, "remote/nas1", "nfs://192.168.1.100/music").await;
    storage::handle_mount_command(&state, "remote/nas2", "smb://server/share").await;

    // List should show both
    let response2 = storage::handle_listmounts_command(&state).await;
    assert!(response2.contains("mount: nfs://192.168.1.100/music"));
    assert!(response2.contains("storage: remote/nas1"));
    assert!(response2.contains("mount: smb://server/share"));
    assert!(response2.contains("storage: remote/nas2"));
    assert!(response2.ends_with("OK\n"));
}

#[tokio::test]
async fn test_protocol_extraction() {
    let state = test_state();

    storage::handle_mount_command(&state, "r1", "nfs://server/path").await;
    storage::handle_mount_command(&state, "r2", "smb://server/share").await;
    storage::handle_mount_command(&state, "r3", "http://server:8080/").await;
    storage::handle_mount_command(&state, "r4", "webdav://server/dav").await;

    let mounts = state.mount_registry.list().await;
    assert_eq!(mounts.len(), 4);

    // Check protocols were extracted correctly
    for mount in mounts {
        match mount.path.as_str() {
            "r1" => assert_eq!(mount.protocol, "nfs"),
            "r2" => assert_eq!(mount.protocol, "smb"),
            "r3" => assert_eq!(mount.protocol, "http"),
            "r4" => assert_eq!(mount.protocol, "webdav"),
            _ => panic!("Unexpected mount path"),
        }
    }
}
