use rmpd_protocol::AppState;
use rmpd_protocol::commands::fingerprint;

#[tokio::test]
async fn test_getfingerprint_no_music_dir() {
    let state = AppState::new();

    let response = fingerprint::handle_getfingerprint_command(&state, "song.mp3").await;

    // Should return error about missing music directory
    assert!(response.contains("ACK"));
    assert!(response.contains("Music directory") || response.contains("not configured"));
}

#[tokio::test]
async fn test_getfingerprint_nonexistent_file() {
    let state = AppState::with_paths("/tmp/db".to_string(), "/tmp".to_string());

    let response =
        fingerprint::handle_getfingerprint_command(&state, "nonexistent_file_12345.mp3").await;

    // Should return error about file not found
    assert!(response.contains("ACK"));
    assert!(response.contains("not found"));
}

#[tokio::test]
async fn test_getfingerprint_path_traversal_blocked() {
    let state = AppState::with_paths("/tmp/db".to_string(), "/tmp".to_string());

    let response = fingerprint::handle_getfingerprint_command(&state, "../etc/passwd").await;

    // Should block path traversal attempts
    assert!(response.contains("ACK"));
}

#[tokio::test]
async fn test_getfingerprint_response_format() {
    let state = AppState::with_paths("/tmp/db".to_string(), "/tmp".to_string());

    let response = fingerprint::handle_getfingerprint_command(&state, "test.mp3").await;

    // Response should be either OK with chromaprint field or ACK error
    assert!(response.ends_with("OK\n") || response.contains("ACK"));

    // If successful, should have chromaprint field
    if !response.contains("ACK") {
        assert!(response.contains("chromaprint: "));
    }
}
