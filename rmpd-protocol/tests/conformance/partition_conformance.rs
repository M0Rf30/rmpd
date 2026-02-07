//! Tests for MPD partition commands over TCP.

use crate::common::tcp_harness::*;

#[tokio::test]
async fn listpartitions_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("listpartitions").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn newpartition_and_delete() {
    let (_server, mut client) = setup().await;

    let resp = client.command("newpartition \"testpart\"").await;
    assert_ok(&resp);

    let resp = client.command("listpartitions").await;
    assert!(
        resp.contains("testpart"),
        "new partition should appear: {resp}"
    );

    let resp = client.command("delpartition \"testpart\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn partition_switch() {
    let (_server, mut client) = setup().await;
    // Create a partition first
    let resp = client.command("newpartition \"otherpart\"").await;
    assert_ok(&resp);

    let resp = client.command("partition \"otherpart\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn delete_nonexistent_partition() {
    let (_server, mut client) = setup().await;
    let resp = client.command("delpartition \"nonexistent\"").await;
    assert!(
        resp.starts_with("ACK "),
        "should error on nonexistent: {resp}"
    );
}

#[tokio::test]
async fn switch_to_nonexistent_partition() {
    let (_server, mut client) = setup().await;
    let resp = client.command("partition \"nonexistent\"").await;
    assert!(resp.starts_with("ACK "), "should error: {resp}");
}

#[tokio::test]
async fn duplicate_newpartition() {
    let (_server, mut client) = setup().await;
    client.command("newpartition \"duppart\"").await;
    let resp = client.command("newpartition \"duppart\"").await;
    assert!(
        resp.starts_with("ACK "),
        "duplicate partition should error: {resp}"
    );
}
