// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Storage and mount conformance tests.
//! Tests mount, unmount, listmounts, and listneighbors.
//!
//! All mount tests use disable_actual_mount to exercise
//! only the registry path (no actual OS mounts).

use crate::tcp_harness::*;
use rmpd_protocol::state::AppState;

#[tokio::test]
async fn listmounts_empty() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let resp = client.command("listmounts").await;
    assert_ok(&resp);
    // No mounts registered yet — response should be just OK
    assert_eq!(resp, "OK\n", "empty listmounts should return bare OK");
}

#[tokio::test]
async fn listneighbors_returns_ok() {
    // Use a state with discovery disabled to avoid mDNS scan timeouts
    let mut state = AppState::new();
    state.discovery = None;
    let (_server, mut client) = setup_with_state(state).await;
    let resp = client.command("listneighbors").await;
    // Should return OK even if no neighbors discovered
    assert_ok(&resp);
}

#[tokio::test]
async fn mount_and_listmounts() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    let resp = client.command("mount \"net\" \"nfs://host/share\"").await;
    assert_ok(&resp);

    let resp = client.command("listmounts").await;
    assert_ok(&resp);
    assert!(
        resp.contains("nfs://host/share"),
        "listmounts should show registered mount: {resp}"
    );
}

#[tokio::test]
async fn unmount_registered() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    client.command("mount \"net\" \"nfs://host/share\"").await;

    let resp = client.command("unmount \"net\"").await;
    assert_ok(&resp);

    // Verify it's gone
    let resp = client.command("listmounts").await;
    assert_eq!(resp, "OK\n", "mount should be removed after unmount");
}

#[tokio::test]
async fn mount_invalid_path() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    // Absolute paths are rejected
    let resp = client.command("mount \"/absolute\" \"nfs://h/s\"").await;
    assert!(
        resp.starts_with("ACK "),
        "mount with absolute path should ACK: {resp}"
    );
    assert!(
        resp.contains("Bad mount point"),
        "mount with absolute path should be a bad mount point: {resp}"
    );
}

#[tokio::test]
async fn mount_nested_path_is_bad_mount_point() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    // MPD only allows top-level mount points (no '/').
    let resp = client.command("mount \"a/b\" \"nfs://h/s\"").await;
    assert_eq!(resp, "ACK [2@0] {mount} Bad mount point\n");
}

#[tokio::test]
async fn mount_busy_and_already_mounted() {
    let (_server, mut client, _tmp) = setup_with_db(1).await;
    assert_ok(&client.command("mount \"net\" \"nfs://h/s\"").await);

    let resp = client.command("mount \"net\" \"nfs://h/other\"").await;
    assert_eq!(resp, "ACK [2@0] {mount} Mount point busy\n");

    let resp = client.command("mount \"net2\" \"nfs://h/s\"").await;
    assert_eq!(resp, "ACK [2@0] {mount} This storage is already mounted\n");
}
