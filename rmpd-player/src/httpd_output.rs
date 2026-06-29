//! Icecast-lite HTTP audio streaming output.
//!
//! Binds a TCP port and streams encoded audio to every connected client.
//! Each new connection receives one HTTP response header and (for WAV) the
//! stream framing header, then gets every subsequent encoded chunk pushed in
//! real time.  Uses only `std::net` — no async runtime.
//!
//! Clients that send `Icy-MetaData: 1` receive a Shoutcast v1 greeting and
//! interleaved ICY metadata blocks every [`ICY_METAINT`] audio bytes.

use crate::audio_output::{AudioOutput, PauseState};
use crate::encoder::{Encoder, PcmEncoder, WavEncoder};
use parking_lot::Mutex;
use rmpd_core::config::OutputConfig;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// ──────────────────────────────────────────────────────────────────────────────

/// The current "now playing" title broadcast to ICY (Shoutcast v1) clients of
/// every httpd output. Updated by the playback engine; read when emitting an
/// ICY metadata block. None = no title (sends an empty/no-update block).
static NOW_PLAYING: StdRwLock<Option<String>> = StdRwLock::new(None);

/// Set the ICY "now playing" title for httpd outputs (call on song change).
pub fn set_now_playing(title: Option<String>) {
    if let Ok(mut g) = NOW_PLAYING.write() {
        *g = title;
    }
}

fn now_playing() -> Option<String> {
    NOW_PLAYING.read().ok().and_then(|g| g.clone())
}

/// Build an ICY "now playing" label from a song: "Artist - Title" when both
/// tags exist, else the title, else the file's base name.
pub fn now_playing_label(song: &rmpd_core::song::Song) -> String {
    let artist = song.tag("artist");
    let title = song.tag("title");
    match (artist, title) {
        (Some(a), Some(t)) => format!("{a} - {t}"),
        (_, Some(t)) => t.to_owned(),
        _ => song.path.file_name().unwrap_or("Unknown").to_owned(),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

/// Audio bytes between ICY metadata blocks (Icecast default).
const ICY_METAINT: usize = 16000;

/// Encode an ICY metadata block: a length byte (count of 16-byte runs) followed
/// by `StreamTitle='...';` zero-padded to that length. `None` → a single 0 byte
/// (no update). Single quotes in the title are stripped so the
/// `StreamTitle='...'` framing cannot be broken; the payload is truncated to
/// the 255*16-byte maximum.
fn icy_meta_block(title: Option<&str>) -> Vec<u8> {
    let t = match title {
        None => return vec![0],
        Some(t) => t,
    };
    let sanitized = t.replace('\'', "");
    let payload = format!("StreamTitle='{sanitized}';");
    let payload_bytes = payload.as_bytes();
    // Truncate to at most 255 * 16 bytes.
    let payload_len = payload_bytes.len().min(255 * 16);
    let runs = payload_len.div_ceil(16);
    let mut block = Vec::with_capacity(1 + runs * 16);
    block.push(runs as u8);
    block.extend_from_slice(&payload_bytes[..payload_len]);
    block.resize(1 + runs * 16, 0);
    block
}

// ──────────────────────────────────────────────────────────────────────────────

/// State for a single connected streaming client.
struct HttpdClient {
    stream: TcpStream,
    /// True when the client sent `Icy-MetaData: 1`; enables interleaving.
    wants_meta: bool,
    /// Audio bytes written since the last metadata block (only meaningful when wants_meta).
    bytes_since_meta: usize,
    /// Last title emitted to this client, to send a no-update block when unchanged.
    last_title: Option<String>,
}

impl HttpdClient {
    /// Write `bytes` to this client, interleaving ICY metadata blocks for meta
    /// clients. Returns `false` if any write fails (caller should drop the client).
    fn serve(&mut self, bytes: &[u8], cur: &Option<String>) -> bool {
        if !self.wants_meta {
            return self.stream.write_all(bytes).is_ok();
        }

        let mut offset = 0;
        while offset < bytes.len() {
            let remaining_to_meta = ICY_METAINT - self.bytes_since_meta;
            let chunk_len = (bytes.len() - offset).min(remaining_to_meta);

            if self
                .stream
                .write_all(&bytes[offset..offset + chunk_len])
                .is_err()
            {
                return false;
            }
            offset += chunk_len;
            self.bytes_since_meta += chunk_len;

            if self.bytes_since_meta == ICY_METAINT {
                let block = if *cur != self.last_title {
                    self.last_title = cur.clone();
                    icy_meta_block(cur.as_deref())
                } else {
                    // No update needed — send the single-zero no-op block.
                    icy_meta_block(None)
                };
                if self.stream.write_all(&block).is_err() {
                    return false;
                }
                self.bytes_since_meta = 0;
            }
        }
        true
    }
}

// ──────────────────────────────────────────────────────────────────────────────

pub struct HttpdOutput {
    addr: String,
    port: u16,
    /// Station name for `icy-name` header; resolved in `new()`.
    name: String,
    /// All currently-connected client streams; dead streams are pruned on write.
    clients: Arc<Mutex<Vec<HttpdClient>>>,
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

        let name = if cfg.name.is_empty() {
            "rmpd".to_owned()
        } else {
            cfg.name.clone()
        };

        Self {
            addr,
            port,
            name,
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

/// Check whether a raw HTTP request head contains `Icy-MetaData: 1`
/// (header name matched case-insensitively, value compared after trimming).
fn has_icy_metadata(request: &[u8]) -> bool {
    let text = match std::str::from_utf8(request) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in text.lines() {
        // "icy-metadata" is 12 ASCII chars; check prefix length first.
        if line.len() > 12 && line[..12].eq_ignore_ascii_case("icy-metadata") {
            if let Some(rest) = line[12..].strip_prefix(':') {
                return rest.trim() == "1";
            }
        }
    }
    false
}

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
        let icy_name = self.name.clone();

        let handle = thread::spawn(move || {
            // HTTP response for plain (non-ICY) clients — identical to the
            // previous behavior, so browsers keep working.
            let http_head = format!(
                "HTTP/1.0 200 OK\r\n\
                 Content-Type: {content_type}\r\n\
                 Connection: close\r\n\
                 Cache-Control: no-cache\r\n\
                 \r\n"
            );
            // Shoutcast v1 response for ICY-capable clients.
            let icy_head = format!(
                "ICY 200 OK\r\n\
                 icy-name: {icy_name}\r\n\
                 icy-pub: 0\r\n\
                 Content-Type: {content_type}\r\n\
                 icy-metaint: {ICY_METAINT}\r\n\
                 \r\n"
            );

            loop {
                if !running.load(Ordering::Acquire) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Read the request head with a short timeout so a silent
                        // client cannot stall this loop indefinitely.
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let wants_meta = {
                            let mut buf: Vec<u8> = Vec::with_capacity(256);
                            let mut tmp = [0u8; 128];
                            let mut found_end = false;
                            while buf.len() < 4096 {
                                match stream.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        buf.extend_from_slice(&tmp[..n]);
                                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                            found_end = true;
                                            break;
                                        }
                                    }
                                    // Timeout or other read error → treat as non-meta.
                                    Err(_) => break,
                                }
                            }
                            found_end && has_icy_metadata(&buf)
                        };

                        let head: &str = if wants_meta { &icy_head } else { &http_head };
                        let ok = stream.write_all(head.as_bytes()).is_ok()
                            && (header_bytes.is_empty() || stream.write_all(&header_bytes).is_ok());
                        if ok {
                            // Clear the read timeout; set a short write timeout so
                            // a slow client cannot block the audio write path.
                            let _ = stream.set_read_timeout(None);
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            // The encoder header (e.g. WAV) is part of the ICY audio
                            // body and counts toward the first metaint boundary.
                            let bytes_since_meta = if wants_meta { header_bytes.len() } else { 0 };
                            clients.lock().push(HttpdClient {
                                stream,
                                wants_meta,
                                bytes_since_meta,
                                last_title: None,
                            });
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
        let cur = now_playing();
        // Prune dead connections in-place; no error is surfaced — dropping a
        // client is normal (e.g. listener navigated away).
        self.clients
            .lock()
            .retain_mut(|client| client.serve(&bytes, &cur));
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
    use std::time::Instant;

    fn make_pcm_output(port: u16) -> HttpdOutput {
        let format = AudioFormat {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        };
        HttpdOutput {
            addr: "127.0.0.1".to_owned(),
            port,
            name: "rmpd".to_owned(),
            clients: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            accept_handle: None,
            encoder: Box::new(PcmEncoder::new(format)),
            bound: None,
            pause_state: PauseState::new(),
        }
    }

    /// Block until at least `want` clients are registered with `output`, or a
    /// generous deadline elapses. The accept thread registers a client only
    /// after it has written that client's greeting header, so once this returns
    /// the connection is guaranteed ready to receive audio from `write`.
    /// Replaces a fixed sleep that can be too short under CI load.
    fn wait_for_clients(output: &HttpdOutput, want: usize) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if output.clients.lock().len() >= want {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Read from `client` until `done(&buf)` holds or a deadline elapses,
    /// returning everything accumulated. The greeting header and the audio
    /// pushed by a later `write` can arrive in separate TCP segments, so a
    /// single `read` may observe only the header — loop until the data lands.
    fn read_until(client: &mut TcpStream, mut done: impl FnMut(&[u8]) -> bool) -> Vec<u8> {
        let start = Instant::now();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        while start.elapsed() < Duration::from_secs(2) {
            match client.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if done(&buf) {
                        break;
                    }
                }
                Err(_) => {
                    if done(&buf) {
                        break;
                    }
                }
            }
        }
        buf
    }

    // ── ICY block builder ──────────────────────────────────────────────────────

    #[test]
    fn icy_meta_block_none_is_single_zero() {
        assert_eq!(icy_meta_block(None), vec![0]);
    }

    #[test]
    fn icy_meta_block_round_trips() {
        let block = icy_meta_block(Some("X"));
        let runs = block[0] as usize;
        assert!(runs > 0, "runs must be non-zero for a title");
        assert_eq!(
            block.len(),
            1 + runs * 16,
            "block length must be 1 + runs*16"
        );
        // Payload must decode back to the original title.
        let title = rmpd_stream::parse_stream_title(&block[1..]);
        assert_eq!(title.as_deref(), Some("X"));
    }

    #[test]
    fn icy_meta_block_sanitizes_single_quotes() {
        let block = icy_meta_block(Some("It's alive"));
        // Apostrophe is stripped → "Its alive".
        let title = rmpd_stream::parse_stream_title(&block[1..]);
        assert_eq!(title.as_deref(), Some("Its alive"));
        // The payload must contain exactly the two framing quotes, no extras.
        let payload = std::str::from_utf8(&block[1..]).unwrap();
        assert_eq!(
            payload.chars().filter(|&c| c == '\'').count(),
            2,
            "payload must have exactly 2 single quotes (the framing pair)"
        );
    }

    // ── Integration: greeting variants ────────────────────────────────────────

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
        // Send a plain request (no Icy-MetaData) so the accept thread can parse
        // and respond immediately without blocking on the read timeout.
        client.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        // Wait until the accept thread has written the greeting and registered
        // the client, so the audio write below is guaranteed to reach it.
        wait_for_clients(&output, 1);

        // Now push a PCM chunk; `clients` contains our stream.
        output.write(&[0.5_f32; 8]).expect("write failed");

        // Read until audio bytes appear after the HTTP header (header and audio
        // may arrive in separate TCP segments).
        let received = read_until(&mut client, |b| {
            b.windows(4)
                .position(|w| w == b"\r\n\r\n")
                .is_some_and(|h| h + 4 < b.len())
        });
        let n = received.len();
        assert!(n > 0, "no data received from httpd output");

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
        // Send a request so the accept thread completes parsing immediately.
        client.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        wait_for_clients(&output, 1);

        output.pause().unwrap();
        output.write(&[0.5_f32; 16]).unwrap();

        // The client should receive the HTTP header but no audio bytes after it,
        // since the write was skipped. Read until the header terminator arrives.
        let received = read_until(&mut client, |b| b.windows(4).any(|w| w == b"\r\n\r\n"));
        let n = received.len();
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

    #[test]
    fn icy_client_receives_shoutcast_greeting() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut icy_client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        icy_client
            .write_all(b"GET / HTTP/1.0\r\nIcy-MetaData: 1\r\n\r\n")
            .unwrap();
        icy_client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        wait_for_clients(&output, 1);
        output.write(&[0.0_f32; 8]).unwrap();

        // Read until the ICY greeting terminator arrives.
        let received = read_until(&mut icy_client, |b| b.windows(4).any(|w| w == b"\r\n\r\n"));

        assert!(
            received.starts_with(b"ICY 200 OK"),
            "expected ICY 200 OK, got: {:?}",
            &received[..received.len().min(32)]
        );
        assert!(
            received
                .windows(b"icy-metaint: 16000".len())
                .any(|w| w.eq_ignore_ascii_case(b"icy-metaint: 16000")),
            "icy-metaint: 16000 header missing from ICY response"
        );

        output.stop().unwrap();
    }

    #[test]
    fn plain_client_receives_http_greeting() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut plain_client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        plain_client.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        plain_client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        wait_for_clients(&output, 1);
        output.write(&[0.0_f32; 8]).unwrap();

        // Read until the HTTP greeting terminator arrives.
        let received = read_until(&mut plain_client, |b| {
            b.windows(4).any(|w| w == b"\r\n\r\n")
        });

        assert!(
            received.starts_with(b"HTTP/1.0 200"),
            "expected HTTP/1.0 200, got: {:?}",
            &received[..received.len().min(32)]
        );
        assert!(
            !received
                .windows(b"icy-metaint".len())
                .any(|w| w.eq_ignore_ascii_case(b"icy-metaint")),
            "icy-metaint must not appear in plain HTTP response"
        );

        output.stop().unwrap();
    }

    #[test]
    fn icy_metadata_interleaved_at_metaint_boundary() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut icy_client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        icy_client
            .write_all(b"GET / HTTP/1.0\r\nIcy-MetaData: 1\r\n\r\n")
            .unwrap();
        icy_client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();

        wait_for_clients(&output, 1);

        // Set a title before writing audio.
        set_now_playing(Some("Test Artist - Test Song".to_owned()));

        // PCM encoder: 1 f32 → 2 bytes; 8000 samples → 16000 bytes = ICY_METAINT.
        // After exactly one metaint block the server emits a metadata block.
        output.write(&vec![0.0_f32; 8000]).unwrap();

        // Drain until the metadata block past the first metaint boundary is
        // fully buffered: HTTP header + ICY_METAINT audio bytes + the length
        // byte and its payload. Stops as soon as the whole block has landed.
        let received = read_until(&mut icy_client, |b| {
            match b.windows(4).position(|w| w == b"\r\n\r\n") {
                Some(h) => {
                    let meta = h + 4 + ICY_METAINT;
                    meta < b.len() && {
                        let runs = b[meta] as usize;
                        runs > 0 && b.len() >= meta + 1 + runs * 16
                    }
                }
                None => false,
            }
        });

        // Locate the ICY response terminator.
        let header_end = received
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("ICY header terminator not found");
        let audio_start = header_end + 4;

        // After ICY_METAINT audio bytes there must be a metadata block.
        let meta_pos = audio_start + ICY_METAINT;
        assert!(
            meta_pos < received.len(),
            "not enough bytes to reach the metadata block boundary \
             (audio_start={audio_start}, meta_pos={meta_pos}, total={})",
            received.len()
        );

        let runs = received[meta_pos] as usize;
        assert!(runs > 0, "metadata length byte must be non-zero");

        let payload_start = meta_pos + 1;
        let payload_end = payload_start + runs * 16;
        assert!(
            payload_end <= received.len(),
            "metadata block payload truncated"
        );

        let title = rmpd_stream::parse_stream_title(&received[payload_start..payload_end]);
        assert_eq!(
            title.as_deref(),
            Some("Test Artist - Test Song"),
            "metadata title mismatch"
        );

        set_now_playing(None);
        output.stop().unwrap();
    }
}
