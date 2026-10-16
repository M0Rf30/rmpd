// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

use rmpd_protocol::AppState;
use rmpd_protocol::commands::storage;

#[tokio::test]
async fn test_listneighbors_with_no_discovery() {
    // Create state without discovery service
    let mut state = AppState::new();
    state.discovery = None;

    // Should return empty OK
    let response = storage::handle_listneighbors_command(&state).await;
    assert_eq!(response, "OK\n");
}

#[tokio::test]
async fn test_listneighbors_with_discovery() {
    let state = AppState::new();

    // Should not panic even if no services found
    let response = storage::handle_listneighbors_command(&state).await;

    // Response should be valid (OK or with neighbors)
    assert!(response.ends_with("OK\n"));
}

#[tokio::test]
async fn test_listneighbors_response_format() {
    let state = AppState::new();

    let response = storage::handle_listneighbors_command(&state).await;

    // Response must end with OK
    assert!(response.ends_with("OK\n"));

    // If neighbors found, should have proper format
    if response != "OK\n" {
        // Check for neighbor field
        assert!(response.contains("neighbor: "));
    }
}

#[tokio::test]
async fn zeroconf_disabled_opens_no_discovery_service() {
    // Regression: the mDNS browser used to be created unconditionally, binding
    // UDP 5353 even with `zeroconf_enabled = false`.
    let mut state = AppState::new();
    assert!(
        state.discovery.is_none(),
        "AppState::new must not open mDNS"
    );

    state.set_zeroconf_enabled(false);
    assert!(state.discovery.is_none());
    assert_eq!(storage::handle_listneighbors_command(&state).await, "OK\n");
}
