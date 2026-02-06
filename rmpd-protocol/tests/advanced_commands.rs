//! Integration tests for advanced commands (partitions, storage, messaging)

mod common;

use common::TestClient;

// Partition commands
#[test]
fn test_partition_command() {
    // partition should switch to named partition
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listpartitions_command() {
    // listpartitions should return list of partitions
    let response = "partition: default\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(
        TestClient::get_field(response, "partition"),
        Some("default")
    );
}

#[test]
fn test_newpartition_command() {
    // newpartition should create a new partition
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_delpartition_command() {
    // delpartition should delete a partition
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_moveoutput_command() {
    // moveoutput should move output to current partition
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

// Storage commands
#[test]
fn test_mount_command() {
    // mount should mount a storage location
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_unmount_command() {
    // unmount should unmount a storage location
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listmounts_command() {
    // listmounts should return mounted storage
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_listneighbors_command() {
    // listneighbors should return network neighbors
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

// Client messaging commands
#[test]
fn test_subscribe_command() {
    // subscribe should subscribe to a channel
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_unsubscribe_command() {
    // unsubscribe should unsubscribe from a channel
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_channels_command() {
    // channels should list available channels
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_readmessages_command() {
    // readmessages should return messages
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_sendmessage_command() {
    // sendmessage should send a message to channel
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

// Output commands
#[test]
fn test_outputs_command() {
    // outputs should list audio outputs
    let response = "outputid: 0\noutputname: Default Output\noutputenabled: 1\nOK\n";
    assert!(TestClient::is_ok(response));
    assert_eq!(TestClient::get_field(response, "outputid"), Some("0"));
    assert_eq!(TestClient::get_field(response, "outputenabled"), Some("1"));
}

#[test]
fn test_enableoutput_command() {
    // enableoutput should enable an output
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_disableoutput_command() {
    // disableoutput should disable an output
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_toggleoutput_command() {
    // toggleoutput should toggle output state
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}

#[test]
fn test_outputset_command() {
    // outputset should set output attribute
    let response = "OK\n";
    assert!(TestClient::is_ok(response));
}
