//! Icecast-lite HTTP audio streaming output.
//!
//! Binds a TCP port and streams encoded audio to every connected client.
//! Each new connection receives one HTTP response header and (for WAV) the
//! stream framing header, then gets every subsequent encoded chunk pushed in
//! real time.  Uses only `std::net` — no async runtime.

use crate::audio_output::{AudioOutput, PauseState};
use crate::encoder::{Encoder, PcmEncoder, WavEncoder};
use parking_lot::Mutex;
use rmpd_core::config::OutputConfig;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// ──────────────────────────────────────────────────────────────────────────────

pub struct HttpdOutput {
    addr: String,
    port: u16,
    /// All currently-connected client streams; dead streams are pruned on write.
    clients: Arc<Mutex<Vec<TcpStream>>>,
    /// Set to `false` by `stop()` to signal the accept thread to exit.
    running: Arc<AtomicBool>,
    accept_handle: Option<JoinHandle<()>>,
    encoder: Box<dyn Encoder>,
    /// Populated after `start()` succeeds; used for ephemeral-port tests.
    bound: Option<SocketAddr>,
    pause_state: PauseState,
}

impl HttpdOutput {
    /// Construct a new `HttpdOutput`.
    ///
    /// Config keys read from `cfg`:
    /// - `bind_to_address` — interface to bind (default `"0.0.0.0"`)
    /// - `port`            — TCP port (default `8000`; `0` = OS-assigned)
    /// - `encoder`         — `"wav"` (default) or `"pcm"`
    pub fn new(format: AudioFormat, cfg: &OutputConfig) -> Self {
        let addr = cfg
            .setting_str("bind_to_address")
            .unwrap_or_else(|| "0.0.0.0".to_owned());

        let port: u16 = cfg
            .setting_str("port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8000);

        let encoder: Box<dyn Encoder> = match cfg.setting_str("encoder").as_deref().unwrap_or("wav")
        {
            "pcm" => Box::new(PcmEncoder::new(format)),
            _ => Box::new(WavEncoder::new(format)),
        };

        Self {
            addr,
            port,
            clients: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            accept_handle: None,
            encoder,
            bound: None,
            pause_state: PauseState::new(),
        }
    }

    /// Returns the bound local address; populated after [`AudioOutput::start`].
    /// Useful for tests that bind on port 0 (OS-assigned ephemeral port).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.bound
    }
}

// ──────────────────────────────────────────────────────────────────────────────

impl AudioOutput for HttpdOutput {
    fn start(&mut self) -> Result<()> {
        use std::net::TcpListener;

        let listener = TcpListener::bind((self.addr.as_str(), self.port)).map_err(|e| {
            RmpdError::Player(format!(
                "httpd: bind {}:{} failed: {e}",
                self.addr, self.port
            ))
        })?;

        self.bound = listener.local_addr().ok();

        listener
            .set_nonblocking(true)
            .map_err(|e| RmpdError::Player(format!("httpd: set_nonblocking failed: {e}")))?;

        self.running.store(true, Ordering::Release);

        // Pre-compute the per-connection preamble so the accept thread needs
        // no reference back to self.
        let running = Arc::clone(&self.running);
        let clients = Arc::clone(&self.clients);
        let content_type = self.encoder.content_type().to_owned();
        let header_bytes = self.encoder.header();

        let handle = thread::spawn(move || {
            let http_head = format!(
                "HTTP/1.0 200 OK\r\n\
                 Content-Type: {content_type}\r\n\
                 Connection: close\r\n\
                 Cache-Control: no-cache\r\n\
                 \r\n"
            );

            loop {
                if !running.load(Ordering::Acquire) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let ok = stream.write_all(http_head.as_bytes()).is_ok()
                            && (header_bytes.is_empty() || stream.write_all(&header_bytes).is_ok());
                        if ok {
                            // Short write timeout: a slow client must not block
                            // the entire audio write path.
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            clients.lock().push(stream);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    // Any other accept error (e.g. listener closed) — exit.
                    Err(_) => break,
                }
            }
        });

        self.accept_handle = Some(handle);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        let bytes = self.encoder.encode(samples);
        // Prune dead connections in-place; no error is surfaced — dropping a
        // client is normal (e.g. listener navigated away).
        self.clients
            .lock()
            .retain_mut(|stream| stream.write_all(&bytes).is_ok());
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.accept_handle.take() {
            // The accept thread wakes at most every 50 ms; join waits one cycle.
            let _ = handle.join();
        }
        self.clients.lock().clear();
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }

    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Build a minimal PCM-encoded `HttpdOutput` without going through
    /// `OutputConfig` (which would require `toml` as a direct dep).
    fn make_pcm_output(port: u16) -> HttpdOutput {
        let format = AudioFormat {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        HttpdOutput {
            addr: "127.0.0.1".to_owned(),
            port,
            clients: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            accept_handle: None,
            encoder: Box::new(PcmEncoder::new(format)),
            bound: None,
            pause_state: PauseState::new(),
        }
    }

    #[test]
    fn httpd_streams_to_client() {
        // port 0 → OS picks an ephemeral port; no collision risk.
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");

        let port = output
            .local_addr()
            .expect("no bound address after start")
            .port();

        // Give the accept thread a moment to enter its loop before connecting.
        thread::sleep(Duration::from_millis(30));

        let addr = format!("127.0.0.1:{port}");
        let mut client = TcpStream::connect(&addr).expect("connect failed");
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        // Allow the accept thread to accept the connection, write the HTTP
        // response header, and push the stream into `clients`.
        thread::sleep(Duration::from_millis(80));

        // Now push a PCM chunk; `clients` contains our stream.
        output.write(&[0.5_f32; 8]).expect("write failed");

        // Read everything the server has pushed so far.
        let mut buf = vec![0u8; 512];
        let n = client.read(&mut buf).unwrap_or(0);
        assert!(n > 0, "no data received from httpd output");

        let received = &buf[..n];

        // Must begin with the HTTP response line.
        assert!(
            received.starts_with(b"HTTP/1.0 200"),
            "expected HTTP/1.0 200, got: {:?}",
            &received[..received.len().min(24)]
        );

        // After the blank line (`\r\n\r\n`) there must be encoded audio bytes.
        let header_end = received
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP header terminator (\\r\\n\\r\\n) not found");
        let audio_start = header_end + 4;
        assert!(
            audio_start < n,
            "no audio bytes after HTTP header (header ends at {audio_start}, total bytes {n})"
        );

        output.stop().expect("stop failed");
    }

    #[test]
    fn paused_output_does_not_write_to_clients() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        thread::sleep(Duration::from_millis(80));

        output.pause().unwrap();
        output.write(&[0.5_f32; 16]).unwrap();

        // The client should receive the HTTP header but no audio bytes after it,
        // since the write was skipped.
        let mut buf = vec![0u8; 512];
        let n = client.read(&mut buf).unwrap_or(0);
        let received = &buf[..n];
        // HTTP header must be there (sent on connect, before pause).
        assert!(received.starts_with(b"HTTP/1.0 200"));
        // But there must be nothing after \r\n\r\n.
        if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            assert_eq!(
                pos + 4,
                n,
                "audio bytes appeared in the buffer despite output being paused"
            );
        }

        output.stop().unwrap();
    }
}
