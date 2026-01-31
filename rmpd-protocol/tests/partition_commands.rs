//! Partition command tests
//!
//! Tests for multi-partition support including partition management,
//! client context switching, and output assignment.

use rmpd_protocol::commands::partition;
use rmpd_protocol::{AppState, ConnectionState};

#[tokio::test]
async fn test_listpartitions_no_manager() {
    let mut state = AppState::new();
    state.partition_manager = None;

    let response = partition::handle_listpartitions_command(&state).await;

    // Should return default partition even without manager
    assert!(response.contains("partition: default"));
    assert!(response.ends_with("OK\n"));
}

#[tokio::test]
async fn test_newpartition_command() {
    let state = AppState::new();

    let response = partition::handle_newpartition_command(&state, "bedroom").await;

    assert_eq!(response, "OK\n");

    // Verify partition was created
    let manager = state.partition_manager.as_ref().unwrap();
    assert!(manager.get_partition("bedroom").await.is_some());
}

#[tokio::test]
async fn test_newpartition_duplicate() {
    let state = AppState::new();

    // Create first partition
    partition::handle_newpartition_command(&state, "bedroom").await;

    // Try to create again
    let response = partition::handle_newpartition_command(&state, "bedroom").await;

    assert!(response.contains("ACK"));
    assert!(response.contains("already exists"));
}

#[tokio::test]
async fn test_newpartition_invalid_name() {
    let state = AppState::new();

    // Test empty name
    let response1 = partition::handle_newpartition_command(&state, "").await;
    assert!(response1.contains("ACK"));
    assert!(response1.contains("Invalid partition name"));

    // Test invalid characters
    let response2 = partition::handle_newpartition_command(&state, "foo/bar").await;
    assert!(response2.contains("ACK"));
    assert!(response2.contains("Invalid partition name"));
}

#[tokio::test]
async fn test_listpartitions_command() {
    let state = AppState::new();

    // Create some partitions
    partition::handle_newpartition_command(&state, "bedroom").await;
    partition::handle_newpartition_command(&state, "kitchen").await;

    let response = partition::handle_listpartitions_command(&state).await;

    assert!(response.contains("partition: bedroom"));
    assert!(response.contains("partition: kitchen"));
    assert!(response.ends_with("OK\n"));
}

#[tokio::test]
async fn test_partition_switch() {
    let state = AppState::new();
    let mut conn_state = ConnectionState::new();

    // Initial partition should be default
    assert_eq!(conn_state.current_partition, "default");

    // Create a new partition
    partition::handle_newpartition_command(&state, "bedroom").await;

    // Switch to it
    let response = partition::handle_partition_command(&state, &mut conn_state, "bedroom").await;

    assert_eq!(response, "OK\n");
    assert_eq!(conn_state.current_partition, "bedroom");
}

#[tokio::test]
async fn test_partition_switch_nonexistent() {
    let state = AppState::new();
    let mut conn_state = ConnectionState::new();

    let response = partition::handle_partition_command(&state, &mut conn_state, "nonexistent").await;

    assert!(response.contains("ACK"));
    assert!(response.contains("No such partition"));
    // Should not change current partition
    assert_eq!(conn_state.current_partition, "default");
}

#[tokio::test]
async fn test_delpartition_command() {
    let state = AppState::new();

    // Create and delete partition
    partition::handle_newpartition_command(&state, "temporary").await;
    let response = partition::handle_delpartition_command(&state, "temporary").await;

    assert_eq!(response, "OK\n");

    // Verify it's gone
    let manager = state.partition_manager.as_ref().unwrap();
    assert!(manager.get_partition("temporary").await.is_none());
}

#[tokio::test]
async fn test_delpartition_default() {
    let state = AppState::new();

    // Should not be able to delete default partition
    let response = partition::handle_delpartition_command(&state, "default").await;

    assert!(response.contains("ACK"));
    assert!(response.contains("Cannot delete default partition"));
}

#[tokio::test]
async fn test_delpartition_nonexistent() {
    let state = AppState::new();

    let response = partition::handle_delpartition_command(&state, "nonexistent").await;

    assert!(response.contains("ACK"));
    // The error message may vary, just check it's an error
    assert!(response.contains("not found") || response.contains("No such partition"));
}

#[tokio::test]
async fn test_moveoutput_no_output() {
    let state = AppState::new();
    let conn_state = ConnectionState::new();

    let response = partition::handle_moveoutput_command(&state, &conn_state, "nonexistent").await;

    assert!(response.contains("ACK"));
    assert!(response.contains("No such output"));
}

#[tokio::test]
async fn test_moveoutput_same_partition() {
    let state = AppState::new();
    let conn_state = ConnectionState::new();

    // Default output exists, moving to default (same partition) should succeed
    let response = partition::handle_moveoutput_command(&state, &conn_state, "Default Output").await;

    // Should succeed (no-op)
    assert_eq!(response, "OK\n");
}

#[tokio::test]
async fn test_partition_isolation() {
    let state = AppState::new();

    // Create two partitions
    partition::handle_newpartition_command(&state, "part1").await;
    partition::handle_newpartition_command(&state, "part2").await;

    let manager = state.partition_manager.as_ref().unwrap();

    // Get both partitions
    let part1 = manager.get_partition("part1").await.unwrap();
    let part2 = manager.get_partition("part2").await.unwrap();

    // Assign outputs differently
    part1.assign_output(0).await;
    part2.assign_output(1).await;

    // Verify isolation
    let part1_outputs = part1.get_outputs().await;
    let part2_outputs = part2.get_outputs().await;

    assert_eq!(part1_outputs.len(), 1);
    assert!(part1_outputs.contains(&0));

    assert_eq!(part2_outputs.len(), 1);
    assert!(part2_outputs.contains(&1));
}

#[tokio::test]
async fn test_multiple_clients_different_partitions() {
    let state = AppState::new();

    // Create partitions
    partition::handle_newpartition_command(&state, "client1_partition").await;
    partition::handle_newpartition_command(&state, "client2_partition").await;

    // Simulate two different clients
    let mut client1 = ConnectionState::new();
    let mut client2 = ConnectionState::new();

    // Each switches to different partition
    partition::handle_partition_command(&state, &mut client1, "client1_partition").await;
    partition::handle_partition_command(&state, &mut client2, "client2_partition").await;

    // Verify they're in different partitions
    assert_eq!(client1.current_partition, "client1_partition");
    assert_eq!(client2.current_partition, "client2_partition");
}

#[tokio::test]
async fn test_output_partition_tracking() {
    let state = AppState::new();

    // Verify default output is assigned to default partition
    let outputs = state.outputs.read().await;
    let default_output = outputs.iter().find(|o| o.id == 0).unwrap();
    assert_eq!(default_output.partition.as_deref(), Some("default"));
    drop(outputs);

    // Create a new partition
    partition::handle_newpartition_command(&state, "bedroom").await;

    // Switch to bedroom partition
    let mut conn_state = ConnectionState::new();
    partition::handle_partition_command(&state, &mut conn_state, "bedroom").await;

    // Move output to bedroom partition
    partition::handle_moveoutput_command(&state, &conn_state, "Default Output").await;

    // Verify OutputInfo reflects new partition ownership
    let outputs = state.outputs.read().await;
    let moved_output = outputs.iter().find(|o| o.id == 0).unwrap();
    assert_eq!(moved_output.partition.as_deref(), Some("bedroom"));
}
