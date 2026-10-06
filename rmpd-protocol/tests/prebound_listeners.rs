// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `MpdServer::run_with_listeners`: serving sockets that were bound by
//! someone else (systemd socket activation) instead of binding our own.

use rmpd_protocol::{AppState, MpdServer};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{broadcast, oneshot};
use tokio::time::timeout;

async fn greeting_and_ping<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    timeout(Duration::from_secs(5), stream.read_line(&mut line))
        .await
        .expect("greeting timed out")
        .unwrap();
    assert!(line.starts_with("OK MPD "), "unexpected greeting: {line:?}");

    stream.get_mut().write_all(b"ping\n").await.unwrap();
    line.clear();
    timeout(Duration::from_secs(5), stream.read_line(&mut line))
        .await
        .expect("ping reply timed out")
        .unwrap();
    assert_eq!(line, "OK\n");
}

#[tokio::test]
async fn serves_prebound_tcp_and_unix_listeners_and_signals_ready() {
    let tcp_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (addr_a, addr_b) = (tcp_a.local_addr().unwrap(), tcp_b.local_addr().unwrap());

    let dir = std::env::temp_dir().join(format!("rmpd-prebound-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock_path = dir.join("mpd.sock");
    let unix = UnixListener::bind(&sock_path).unwrap();

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let (ready_tx, ready_rx) = oneshot::channel();
    // The configured bind address / unix socket must be ignored entirely.
    let server = MpdServer::with_state("127.0.0.1:1".to_string(), AppState::new(), shutdown_rx)
        .with_unix_socket(Some(dir.join("must-not-be-created").display().to_string()))
        .with_ready_signal(ready_tx);
    let handle = tokio::spawn(server.run_with_listeners(vec![tcp_a, tcp_b], vec![unix]));

    timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("ready signal timed out")
        .expect("ready signal dropped");

    greeting_and_ping(TcpStream::connect(addr_a).await.unwrap()).await;
    greeting_and_ping(TcpStream::connect(addr_b).await.unwrap()).await;
    greeting_and_ping(UnixStream::connect(&sock_path).await.unwrap()).await;
    assert!(!dir.join("must-not-be-created").exists());

    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(5), handle)
        .await
        .expect("server did not stop")
        .unwrap()
        .unwrap();

    // Inherited sockets belong to the supervisor: the file must survive us.
    assert!(sock_path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn ready_signal_fires_for_bound_listener_entry_point() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let (ready_tx, ready_rx) = oneshot::channel();
    let server = MpdServer::with_state("unused".to_string(), AppState::new(), shutdown_rx)
        .with_ready_signal(ready_tx);
    let handle = tokio::spawn(server.run_with_listener(listener));

    timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("ready signal timed out")
        .expect("ready signal dropped");
    greeting_and_ping(TcpStream::connect(addr).await.unwrap()).await;

    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(5), handle)
        .await
        .expect("server did not stop")
        .unwrap()
        .unwrap();
}
