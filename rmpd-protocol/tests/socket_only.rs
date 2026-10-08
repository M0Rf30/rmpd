//! Regression tests for socket-only mode (empty `bind_address`).

/// A bind address naming a path is a socket, matching MPD's `bind_to_address`.
#[test]
fn socket_paths_are_recognised() {
    use rmpd_protocol::server::is_socket_path;

    assert!(is_socket_path("/run/user/1000/rmpd.sock"));
    assert!(is_socket_path("~/rmpd.sock"));
    assert!(!is_socket_path("127.0.0.1"));
    assert!(!is_socket_path("::1"));
    assert!(!is_socket_path("localhost"));
    assert!(!is_socket_path(""));
}

/// A path in `bind_address` wins, and no TCP listener is configured.
#[test]
fn socket_path_selects_socket_only() {
    use rmpd_protocol::server::socket_only_path;

    assert_eq!(
        socket_only_path("/tmp/rmpd.sock", None).unwrap(),
        Some("/tmp/rmpd.sock".to_string())
    );
    // an address means normal TCP service
    assert_eq!(socket_only_path("127.0.0.1", None).unwrap(), None);
}

/// An empty `bind_address` takes its path from `unix_socket`.
#[test]
fn empty_bind_address_uses_the_unix_socket_key() {
    use rmpd_protocol::server::socket_only_path;

    assert_eq!(
        socket_only_path("", Some("/tmp/rmpd.sock")).unwrap(),
        Some("/tmp/rmpd.sock".to_string())
    );
}

/// With neither configured, there is nothing to serve and startup must fail
/// rather than bind a port the user did not ask for.
#[test]
fn empty_bind_address_without_unix_socket_errors() {
    use rmpd_protocol::server::socket_only_path;

    let err = socket_only_path("", None).expect_err("empty address must be rejected");
    assert!(
        err.to_string().contains("nothing to listen on"),
        "unexpected error: {err}"
    );
}

/// A socket path whose parent directory does not exist must be reported by
/// name, instead of surfacing the bare `ENOENT` from `bind()`.
#[tokio::test]
async fn missing_socket_parent_directory_is_reported() {
    use rmpd_protocol::server::MpdServer;
    use rmpd_protocol::state::AppState;
    use tokio::sync::broadcast;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let sock_path = temp.path().join("nope").join("rmpd.sock"); // parent "nope" is absent

    let (_shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    let server = MpdServer::with_state(String::new(), AppState::new(), shutdown_rx);

    let err = server
        .run_unix_socket(sock_path.to_string_lossy().to_string())
        .await
        .expect_err("missing parent directory must fail");

    let msg = err.to_string();
    assert!(msg.contains("does not exist"), "unexpected error: {msg}");
}

/// The address the server binds: ports are appended to TCP addresses only.
#[test]
fn resolve_bind_address_shape() {
    use rmpd_protocol::server::resolve_bind_address;

    assert_eq!(resolve_bind_address("127.0.0.1", 6600), "127.0.0.1:6600");
    assert_eq!(resolve_bind_address("::1", 6600), "[::1]:6600");
    assert_eq!(resolve_bind_address("[::1]", 6600), "[::1]:6600");
    assert_eq!(resolve_bind_address("localhost", 6600), "localhost:6600");

    // a socket path and an empty address must reach the server untouched,
    // otherwise they end up looking like addresses and cannot be bound
    assert_eq!(
        resolve_bind_address("/run/user/1000/rmpd.sock", 6600),
        "/run/user/1000/rmpd.sock"
    );
    assert_eq!(resolve_bind_address("", 6600), "");
}

/// Socket-only mode serves a socket and binds no TCP port.
///
/// Exercises the accept loop, not just the policy: the server starts with no TCP
/// listener, so the socket has to appear, answer with the greeting, and shut
/// down when the broadcast fires.
#[tokio::test]
async fn socket_only_server_serves_a_socket() {
    use rmpd_protocol::server::MpdServer;
    use rmpd_protocol::state::AppState;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::sync::broadcast;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let sock = temp.path().join("rmpd.sock");

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    let server = MpdServer::with_state(String::new(), AppState::new(), shutdown_rx);
    let path = sock.to_string_lossy().to_string();
    let handle = tokio::spawn(async move { server.run_unix_socket(path).await });

    let mut greeting = String::new();
    for _ in 0..40 {
        if let Ok(stream) = tokio::net::UnixStream::connect(&sock).await {
            let mut reader = BufReader::new(stream);
            if tokio::time::timeout(
                std::time::Duration::from_secs(2),
                reader.read_line(&mut greeting),
            )
            .await
            .is_ok()
            {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        greeting.starts_with("OK MPD"),
        "no greeting, got {greeting:?}"
    );

    let _ = shutdown_tx.send(());
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(
        matches!(stopped, Ok(Ok(Ok(())))),
        "server did not stop cleanly: {stopped:?}"
    );
}

/// `~` is expanded for a socket path, the same way the other paths are.
#[test]
fn tilde_in_a_socket_path_is_expanded() {
    use rmpd_protocol::server::socket_only_path;

    let home = std::env::var("HOME").expect("HOME");
    let resolved = socket_only_path("~/rmpd.sock", None)
        .expect("a home path is valid")
        .expect("names a socket");

    assert_eq!(resolved, format!("{home}/rmpd.sock"));
    assert!(!resolved.starts_with('~'), "left unexpanded: {resolved}");
}

/// A missing directory is reported against the key the path came from.
#[test]
fn missing_directory_names_the_key_in_use() {
    use rmpd_protocol::server::socket_only_path;

    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("nope").join("rmpd.sock");
    let err = socket_only_path(path.to_str().unwrap(), None)
        .expect_err("missing parent must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("network.bind_address") && msg.contains("does not exist"),
        "unexpected error: {msg}"
    );
}
