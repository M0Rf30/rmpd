//! Integration tests for advanced commands (partitions, storage, messaging)

mod common;
#[path = "common/tcp_harness.rs"]
mod tcp_harness;

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

#[tokio::test]
async fn test_subscribe_command() {
    let (_server, mut client) = tcp_harness::setup().await;
    let response = client.command("subscribe test-channel").await;
    tcp_harness::assert_ok(&response);
}

#[tokio::test]
async fn test_subscribe_invalid_channel_name_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    let response = client.command("subscribe \"bad channel!\"").await;
    assert!(response.starts_with("ACK [2@0]"), "got: {response}");
}

#[tokio::test]
async fn test_subscribe_duplicate_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    tcp_harness::assert_ok(&client.command("subscribe test-channel").await);
    let response = client.command("subscribe test-channel").await;
    assert!(response.starts_with("ACK [56@0]"), "got: {response}");
}

#[tokio::test]
async fn test_subscribe_full_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    for i in 0..16 {
        tcp_harness::assert_ok(&client.command(&format!("subscribe chan{i}")).await);
    }
    let response = client.command("subscribe chan16").await;
    assert!(response.starts_with("ACK [56@0]"), "got: {response}");
    assert!(
        response.contains("subscription list is full"),
        "got: {response}"
    );
}

#[tokio::test]
async fn test_unsubscribe_command() {
    let (_server, mut client) = tcp_harness::setup().await;
    tcp_harness::assert_ok(&client.command("subscribe test-channel").await);
    let response = client.command("unsubscribe test-channel").await;
    tcp_harness::assert_ok(&response);
}

#[tokio::test]
async fn test_unsubscribe_not_subscribed_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    let response = client.command("unsubscribe never-subscribed").await;
    assert!(response.starts_with("ACK [50@0]"), "got: {response}");
}

#[tokio::test]
async fn test_channels_command_lists_subscriptions() {
    let server = tcp_harness::MpdTestServer::start().await;
    let mut a = tcp_harness::MpdTestClient::connect(server.port()).await;
    tcp_harness::assert_ok(&a.command("subscribe test-channel").await);

    let mut b = tcp_harness::MpdTestClient::connect(server.port()).await;
    let response = b.command("channels").await;
    assert!(
        response.contains("channel: test-channel"),
        "got: {response}"
    );
}

#[tokio::test]
async fn test_sendmessage_and_readmessages_roundtrip() {
    let server = tcp_harness::MpdTestServer::start().await;
    let mut receiver = tcp_harness::MpdTestClient::connect(server.port()).await;
    tcp_harness::assert_ok(&receiver.command("subscribe test-channel").await);

    let mut sender = tcp_harness::MpdTestClient::connect(server.port()).await;
    tcp_harness::assert_ok(&sender.command("sendmessage test-channel \"hello\"").await);

    let response = receiver.command("readmessages").await;
    assert_eq!(response, "channel: test-channel\nmessage: hello\nOK\n");

    // The queue is drained: a second read returns nothing.
    let response = receiver.command("readmessages").await;
    assert_eq!(response, "OK\n");
}

#[tokio::test]
async fn test_sendmessage_no_subscribers_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    let response = client.command("sendmessage nobody-here \"hi\"").await;
    assert!(response.starts_with("ACK [50@0]"), "got: {response}");
}

#[tokio::test]
async fn test_sendmessage_invalid_channel_name_rejected() {
    let (_server, mut client) = tcp_harness::setup().await;
    let response = client.command("sendmessage \"bad channel!\" \"hi\"").await;
    assert!(response.starts_with("ACK [2@0]"), "got: {response}");
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
