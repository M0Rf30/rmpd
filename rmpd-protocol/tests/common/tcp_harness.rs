//! TCP-level test harness for MPD protocol conformance tests.
//!
//! Provides `MpdTestServer` (binds to port 0, spawns the real server) and
//! `MpdTestClient` (connects via TCP, sends commands, validates responses).

use rmpd_core::song::Song;
use rmpd_protocol::MpdServer;
use rmpd_protocol::state::AppState;
use std::time::Duration as StdDuration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A test MPD server bound to a random OS-assigned port.
pub struct MpdTestServer {
    port: u16,
    shutdown_tx: broadcast::Sender<()>,
}

impl MpdTestServer {
    /// Start a server with default (empty) state.
    pub async fn start() -> Self {
        Self::start_with_state(AppState::new()).await
    }

    /// Start a server with pre-configured state.
    pub async fn start_with_state(mut state: AppState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        state.set_shutdown_sender(shutdown_tx.clone());

        let server = MpdServer::with_state(format!("127.0.0.1:{port}"), state, shutdown_rx);

        tokio::spawn(async move {
            let _ = server.run_with_listener(listener).await;
        });

        // Give the server a moment to be ready.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self { port, shutdown_tx }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for MpdTestServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

/// A TCP client that speaks the MPD protocol.
pub struct MpdTestClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl MpdTestClient {
    /// Connect to a running test server and consume the greeting line.
    pub async fn connect(port: u16) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.set_nodelay(true).unwrap();

        let (read_half, write_half) = stream.into_split();
        let mut client = Self {
            reader: BufReader::new(read_half),
            writer: write_half,
        };

        // Read and validate greeting
        let greeting = client.read_line().await;
        assert!(
            greeting.starts_with("OK MPD "),
            "unexpected greeting: {greeting}"
        );

        client
    }

    /// Send a single command and read the full response (up to OK or ACK).
    pub async fn command(&mut self, cmd: &str) -> String {
        self.send_raw(&format!("{cmd}\n")).await;
        self.read_response().await
    }

    /// Send a command list (command_list_begin / end) and read the response.
    pub async fn command_list(&mut self, cmds: &[&str]) -> String {
        let mut payload = String::from("command_list_begin\n");
        for cmd in cmds {
            payload.push_str(cmd);
            payload.push('\n');
        }
        payload.push_str("command_list_end\n");
        self.send_raw(&payload).await;
        self.read_response().await
    }

    /// Send a command list with OK separators and read the response.
    pub async fn command_list_ok(&mut self, cmds: &[&str]) -> String {
        let mut payload = String::from("command_list_ok_begin\n");
        for cmd in cmds {
            payload.push_str(cmd);
            payload.push('\n');
        }
        payload.push_str("command_list_end\n");
        self.send_raw(&payload).await;
        self.read_response().await
    }

    /// Send raw bytes to the server.
    pub async fn send_raw(&mut self, data: &str) {
        self.writer.write_all(data.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    /// Read a single line (including the trailing newline).
    pub async fn read_line(&mut self) -> String {
        let mut line = String::new();
        timeout(READ_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .expect("read_line timed out")
            .expect("read_line IO error");
        line
    }

    /// Read lines until we see `OK\n` or a line starting with `ACK`.
    pub async fn read_response(&mut self) -> String {
        let mut response = String::new();
        loop {
            let line = self.read_line().await;
            if line.is_empty() {
                // Connection closed unexpectedly.
                break;
            }
            response.push_str(&line);
            if line == "OK\n" || line.starts_with("ACK ") {
                break;
            }
        }
        response
    }
}

// ── Static assertion helpers ─────────────────────────────────────────

/// Assert the response ends with `OK\n`.
pub fn assert_ok(response: &str) {
    assert!(
        response.ends_with("OK\n"),
        "expected OK response, got: {response}"
    );
}

/// Extract the value of a `key: value` field from a response.
pub fn get_field<'a>(response: &'a str, field: &str) -> Option<&'a str> {
    let prefix = format!("{field}: ");
    response
        .lines()
        .find(|line| line.starts_with(&prefix))
        .map(|line| &line[prefix.len()..])
}

// ── Convenience setup functions ──────────────────────────────────────

/// Create a server and a single connected client.
pub async fn setup() -> (MpdTestServer, MpdTestClient) {
    let server = MpdTestServer::start().await;
    let client = MpdTestClient::connect(server.port()).await;
    (server, client)
}

/// Create a server with custom state and a single connected client.
pub async fn setup_with_state(state: AppState) -> (MpdTestServer, MpdTestClient) {
    let server = MpdTestServer::start_with_state(state).await;
    let client = MpdTestClient::connect(server.port()).await;
    (server, client)
}

/// Create a test song with the given path and track number.
pub fn make_test_song(path: &str, track: u32) -> Song {
    Song {
        id: track as u64,
        path: path.into(),
        duration: Some(StdDuration::from_secs(180)),
        title: Some(format!("Track {track}")),
        artist: Some("Test Artist".to_string()),
        album: Some("Test Album".to_string()),
        album_artist: None,
        track: Some(track),
        disc: None,
        date: Some("2024".to_string()),
        genre: Some("Rock".to_string()),
        composer: None,
        performer: None,
        comment: None,
        musicbrainz_trackid: None,
        musicbrainz_albumid: None,
        musicbrainz_artistid: None,
        musicbrainz_albumartistid: None,
        musicbrainz_releasegroupid: None,
        musicbrainz_releasetrackid: None,
        artist_sort: None,
        album_artist_sort: None,
        original_date: None,
        label: None,
        sample_rate: Some(44100),
        channels: Some(2),
        bits_per_sample: Some(16),
        bitrate: Some(320),
        replay_gain_track_gain: None,
        replay_gain_track_peak: None,
        replay_gain_album_gain: None,
        replay_gain_album_peak: None,
        added_at: 0,
        last_modified: 0,
    }
}

/// Create a server backed by a temporary SQLite database pre-populated with
/// test songs, plus a connected client. Returns the TempDir so it stays alive.
pub async fn setup_with_db(num_songs: u32) -> (MpdTestServer, MpdTestClient, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();
    let music_dir = tmp.path().join("music");
    std::fs::create_dir_all(&music_dir).unwrap();

    // Populate the database
    {
        let db = rmpd_library::Database::open(&db_path_str).unwrap();
        for i in 1..=num_songs {
            let song = make_test_song(&format!("music/song{i}.flac"), i);
            db.add_song(&song).unwrap();
        }
    }

    let mut state = AppState::with_paths(db_path_str, music_dir.to_str().unwrap().to_string());
    state.disable_actual_mount = true;
    let server = MpdTestServer::start_with_state(state).await;
    let client = MpdTestClient::connect(server.port()).await;
    (server, client, tmp)
}
