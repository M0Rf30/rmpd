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
