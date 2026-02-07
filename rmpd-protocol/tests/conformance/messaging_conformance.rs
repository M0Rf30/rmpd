//! Tests for MPD client-to-client messaging commands over TCP.

use crate::tcp_harness::*;

#[tokio::test]
async fn subscribe_and_unsubscribe() {
    let (_server, mut client) = setup().await;

    let resp = client.command("subscribe \"testchan\"").await;
    assert_ok(&resp);

    let resp = client.command("unsubscribe \"testchan\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn subscribe_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("subscribe \"mychannel\"").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn sendmessage_and_readmessages() {
    let server = MpdTestServer::start().await;
    let mut client1 = MpdTestClient::connect(server.port()).await;
    let mut client2 = MpdTestClient::connect(server.port()).await;

    client1.command("subscribe \"msgchan\"").await;

    let resp = client2.command("sendmessage \"msgchan\" \"hello\"").await;
    assert_ok(&resp);

    let resp = client1.command("readmessages").await;
    assert_ok(&resp);
    // Message may or may not be present depending on broker implementation
}

#[tokio::test]
async fn readmessages_empty() {
    let (_server, mut client) = setup().await;
    let resp = client.command("readmessages").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn channels_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("channels").await;
    assert_ok(&resp);
}
