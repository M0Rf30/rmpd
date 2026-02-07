//! Tests for MPD connection lifecycle: greeting, ping, close, empty lines,
//! unknown commands, concurrent clients, abrupt disconnect.

use crate::tcp_harness::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::Duration;

#[tokio::test]
async fn greeting_format() {
    let server = MpdTestServer::start().await;
    let stream = TcpStream::connect(("127.0.0.1", server.port()))
        .await
        .unwrap();
    let (read_half, _write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();

    assert!(
        line.starts_with("OK MPD "),
        "greeting must start with 'OK MPD '"
    );
    // Version should be numeric like "0.24.0"
    let version = line.trim().strip_prefix("OK MPD ").unwrap();
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "version must be major.minor.patch");
    for part in &parts {
        assert!(
            part.parse::<u32>().is_ok(),
            "version part not numeric: {part}"
        );
    }
}

#[tokio::test]
async fn ping_returns_ok() {
    let (_server, mut client) = setup().await;
    let resp = client.command("ping").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn close_terminates_connection() {
    let (_server, mut client) = setup().await;
    client.send_raw("close\n").await;
    // Per MPD spec, "close" terminates the connection without a response.
    let line = client.read_line().await;
    assert!(line.is_empty(), "expected connection closed, got: {line}");
}

#[tokio::test]
async fn empty_lines_are_ignored() {
    let (_server, mut client) = setup().await;
    // Send empty lines then a real command
    client.send_raw("\n\n\nping\n").await;
    let resp = client.read_response().await;
    assert_ok(&resp);
}

#[tokio::test]
async fn unknown_command_returns_ack() {
    let (_server, mut client) = setup().await;
    let resp = client.command("this_is_not_a_real_command").await;
    assert!(resp.starts_with("ACK "), "expected ACK for unknown command");
}

#[tokio::test]
async fn concurrent_clients() {
    let server = MpdTestServer::start().await;
    let mut client1 = MpdTestClient::connect(server.port()).await;
    let mut client2 = MpdTestClient::connect(server.port()).await;

    let resp1 = client1.command("ping").await;
    let resp2 = client2.command("ping").await;

    assert_ok(&resp1);
    assert_ok(&resp2);
}

#[tokio::test]
async fn abrupt_disconnect_does_not_crash_server() {
    let server = MpdTestServer::start().await;

    // Connect and immediately drop
    {
        let stream = TcpStream::connect(("127.0.0.1", server.port()))
            .await
            .unwrap();
        drop(stream);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Server should still accept new connections
    let mut client = MpdTestClient::connect(server.port()).await;
    let resp = client.command("ping").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn multiple_commands_on_same_connection() {
    let (_server, mut client) = setup().await;

    for _ in 0..10 {
        let resp = client.command("ping").await;
        assert_ok(&resp);
    }
}

#[tokio::test]
async fn password_accepted() {
    let (_server, mut client) = setup().await;
    // rmpd doesn't enforce passwords, so any password should succeed
    let resp = client.command("password mypassword").await;
    assert_ok(&resp);
}

#[tokio::test]
async fn rapid_connect_disconnect() {
    let server = MpdTestServer::start().await;

    for _ in 0..20 {
        let stream = TcpStream::connect(("127.0.0.1", server.port()))
            .await
            .unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("OK MPD "));
        write_half.write_all(b"close\n").await.unwrap();
    }

    // Server should still work
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut client = MpdTestClient::connect(server.port()).await;
    let resp = client.command("ping").await;
    assert_ok(&resp);
}
