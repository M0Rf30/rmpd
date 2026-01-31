//! Common test utilities for integration tests

use rmpd_protocol::server::MpdServer;
use rmpd_protocol::state::AppState;
use std::net::TcpStream;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use tokio::sync::broadcast;

/// Test helper for MPD protocol commands
pub struct TestClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl TestClient {
    /// Connect to the test server
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let reader_stream = stream.try_clone()?;
        let reader = BufReader::new(reader_stream);

        let mut client = Self { stream, reader };

        // Read and discard the initial "OK MPD" greeting
        client.read_response()?;

        Ok(client)
    }

    /// Send a command and return the response
    pub fn command(&mut self, cmd: &str) -> std::io::Result<String> {
        writeln!(self.stream, "{}", cmd)?;
        self.stream.flush()?;
        self.read_response()
    }

    /// Read a response until OK or ACK
    fn read_response(&mut self) -> std::io::Result<String> {
        let mut response = String::new();
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line)?;

            if line.starts_with("OK") || line.starts_with("ACK") {
                response.push_str(&line);
                break;
            }
            response.push_str(&line);
        }
        Ok(response)
    }

    /// Check if response is OK
    pub fn is_ok(response: &str) -> bool {
        response.trim().ends_with("OK")
    }

    /// Check if response is an error
    pub fn is_error(response: &str) -> bool {
        response.starts_with("ACK")
    }

    /// Extract field value from response
    pub fn get_field<'a>(response: &'a str, field: &str) -> Option<&'a str> {
        let prefix = format!("{}: ", field);
        response
            .lines()
            .find(|line| line.starts_with(&prefix))
            .map(|line| line.trim_start_matches(&prefix))
    }
}

/// Start a test server on a random port
pub async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Use port 0 to get a random available port
    let bind_address = "127.0.0.1:0".to_string();

    // Create server
    let server = MpdServer::new(bind_address.clone(), shutdown_rx);

    // Start server in background
    let handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            eprintln!("Server error: {}", e);
        }
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get the actual bound address
    // For now, use a fixed test port since we can't easily get the dynamic port
    let test_addr = "127.0.0.1:16600".to_string();

    (test_addr, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ok() {
        assert!(TestClient::is_ok("OK\n"));
        assert!(TestClient::is_ok("field: value\nOK\n"));
        assert!(!TestClient::is_ok("ACK [5@0] {command} error\n"));
    }

    #[test]
    fn test_is_error() {
        assert!(TestClient::is_error("ACK [5@0] {command} error\n"));
        assert!(!TestClient::is_error("OK\n"));
    }

    #[test]
    fn test_get_field() {
        let response = "volume: 100\nrepeat: 0\nOK\n";
        assert_eq!(TestClient::get_field(response, "volume"), Some("100"));
        assert_eq!(TestClient::get_field(response, "repeat"), Some("0"));
        assert_eq!(TestClient::get_field(response, "missing"), None);
    }
}
