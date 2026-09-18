//! Icecast-lite HTTP audio streaming output.
//!
//! Binds a TCP port and streams encoded audio to every connected client.
//! Each new connection receives one HTTP response header and (for WAV) the
//! stream framing header, then gets every subsequent encoded chunk pushed in
//! real time.  Uses only `std::net` — no async runtime.
//!
//! Clients that send `Icy-MetaData: 1` receive a Shoutcast v1 greeting and
//! interleaved ICY metadata blocks every `ICY_METAINT` audio bytes.

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
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Bounded per-client outgoing-chunk queue depth. A client whose socket
/// cannot keep up (queue full) is dropped rather than stalling the audio
/// thread that calls `write` (SEC-07).
const CLIENT_QUEUE_DEPTH: usize = 8;

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

/// State for a single connected streaming client: a handle to its dedicated
/// writer thread. Audio bytes are hand off via a bounded channel so a slow
/// or stalled socket can never block the caller of `write` (the realtime
/// audio thread) — see the module doc.
struct HttpdClient {
    tx: SyncSender<Arc<[u8]>>,
}

/// Write `bytes` to `stream`, interleaving ICY metadata blocks when
/// `wants_meta` is set. Returns `false` if any write fails (caller drops the
/// client). Runs on the client's dedicated writer thread.
fn serve_chunk(
    stream: &mut TcpStream,
    bytes: &[u8],
    wants_meta: bool,
    bytes_since_meta: &mut usize,
    cur: &Option<String>,
    last_title: &mut Option<String>,
) -> bool {
    if !wants_meta {
        return stream.write_all(bytes).is_ok();
    }

    let mut offset = 0;
    while offset < bytes.len() {
        let remaining_to_meta = ICY_METAINT - *bytes_since_meta;
        let chunk_len = (bytes.len() - offset).min(remaining_to_meta);

        if stream
            .write_all(&bytes[offset..offset + chunk_len])
            .is_err()
        {
            return false;
        }
        offset += chunk_len;
        *bytes_since_meta += chunk_len;

        if *bytes_since_meta == ICY_METAINT {
            let block = if *cur != *last_title {
                *last_title = cur.clone();
                icy_meta_block(cur.as_deref())
            } else {
                // No update needed — send the single-zero no-op block.
                icy_meta_block(None)
            };
            if stream.write_all(&block).is_err() {
                return false;
            }
            *bytes_since_meta = 0;
        }
    }
    true
}

/// Spawn the dedicated writer thread for one accepted client. Returns the
/// bounded sender used to hand off encoded audio chunks; the thread exits
/// once the sender is dropped or a write fails.
fn spawn_client_writer(
    mut stream: TcpStream,
    wants_meta: bool,
    mut bytes_since_meta: usize,
) -> SyncSender<Arc<[u8]>> {
    let (tx, rx) = sync_channel::<Arc<[u8]>>(CLIENT_QUEUE_DEPTH);
    thread::spawn(move || {
        let mut last_title: Option<String> = None;
        while let Ok(bytes) = rx.recv() {
            let cur = now_playing();
            if !serve_chunk(
                &mut stream,
                &bytes,
                wants_meta,
                &mut bytes_since_meta,
                &cur,
                &mut last_title,
            ) {
                break;
            }
        }
    });
    tx
}

// ──────────────────────────────────────────────────────────────────────────────

pub struct HttpdOutput {
    addr: String,
    port: u16,
    /// Station name for `icy-name` header; resolved in `new()`.
    name: String,
    /// All currently-connected client streams; dead streams are pruned on write.
    clients: Arc<Mutex<Vec<HttpdClient>>>,
    /// Maximum simultaneously-connected clients; new connections beyond this
    /// are refused (SEC-07: unbounded clients exhaust the daemon's fds).
    max_clients: usize,
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
    /// - `bind_to_address` — interface to bind (default `"127.0.0.1"`; set
    ///   explicitly to `"0.0.0.0"` to expose the stream off-host)
    /// - `port`            — TCP port (default `8000`; `0` = OS-assigned)
    /// - `encoder`         — `"wav"` (default) or `"pcm"`
    /// - `max_clients`     — simultaneous client cap (default `32`)
    pub fn new(format: AudioFormat, cfg: &OutputConfig) -> Self {
        let addr = cfg
            .setting_str("bind_to_address")
            .unwrap_or_else(|| "127.0.0.1".to_owned());

        let port: u16 = cfg
            .setting_str("port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8000);

        let max_clients: usize = cfg
            .setting_str("max_clients")
            .and_then(|s| s.parse().ok())
            .unwrap_or(32);

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
            max_clients,
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
        if line.len() > 12
            && line[..12].eq_ignore_ascii_case("icy-metadata")
            && let Some(rest) = line[12..].strip_prefix(':')
        {
            return rest.trim() == "1";
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
        let max_clients = self.max_clients;
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
                    Ok((stream, _)) if clients.lock().len() >= max_clients => {
                        // SEC-07: cap concurrent clients so a slow-loris of
                        // connections cannot exhaust the daemon's fds. Refuse
                        // before spending the handshake budget on it.
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                    Ok((mut stream, _)) => {
                        // Poll for the request head with a short per-read timeout so a
                        // silent client cannot stall this loop, but keep polling until an
                        // overall deadline: the head can arrive late, or split across TCP
                        // segments, and treating the first WouldBlock as "no header" would
                        // silently downgrade an ICY client to a plain HTTP greeting.
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let wants_meta = {
                            let deadline = Instant::now() + Duration::from_secs(2);
                            let mut buf: Vec<u8> = Vec::with_capacity(256);
                            let mut tmp = [0u8; 128];
                            let mut found_end = false;
                            while buf.len() < 4096 && Instant::now() < deadline {
                                match stream.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        buf.extend_from_slice(&tmp[..n]);
                                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                            found_end = true;
                                            break;
                                        }
                                    }
                                    // A read timeout just means the rest of the head has
                                    // not arrived yet; keep waiting until the deadline.
                                    Err(e)
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                                | std::io::ErrorKind::Interrupted
                                        ) => {}
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
                            // a slow client's handshake write cannot block long.
                            let _ = stream.set_read_timeout(None);
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            // The encoder header (e.g. WAV) is part of the ICY audio
                            // body and counts toward the first metaint boundary.
                            let bytes_since_meta = if wants_meta { header_bytes.len() } else { 0 };
                            let tx = spawn_client_writer(stream, wants_meta, bytes_since_meta);
                            clients.lock().push(HttpdClient { tx });
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
        let bytes: Arc<[u8]> = Arc::from(self.encoder.encode(samples));
        // Non-blocking hand-off: a client whose bounded queue is full (too
        // slow to keep up) or whose writer thread has exited is dropped
        // rather than stalling this call — the realtime audio thread must
        // never block on a client socket (SEC-07).
        self.clients
            .lock()
            .retain(|client| client.tx.try_send(Arc::clone(&bytes)).is_ok());
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
            max_clients: 32,
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

    /// A request head that arrives in two segments separated by more than the
    /// accept loop's per-read timeout must still be recognised as ICY. The read
    /// loop used to treat its first timeout as "no more headers" and downgrade
    /// the client to a plain HTTP greeting, which is how this surfaced: as a
    /// load-dependent CI failure on all four test legs at once.
    #[test]
    fn icy_greeting_survives_split_request_head() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut icy_client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        icy_client
            .set_read_timeout(Some(Duration::from_millis(3000)))
            .unwrap();

        // First segment only: no terminator, no Icy-MetaData header yet.
        icy_client.write_all(b"GET / HTTP/1.0\r\n").unwrap();
        // Longer than the accept loop's 200ms per-read timeout.
        thread::sleep(Duration::from_millis(350));
        icy_client.write_all(b"Icy-MetaData: 1\r\n\r\n").unwrap();

        wait_for_clients(&output, 1);
        output.write(&[0.0_f32; 8]).unwrap();

        let received = read_until(&mut icy_client, |b| b.windows(4).any(|w| w == b"\r\n\r\n"));

        assert!(
            received.starts_with(b"ICY 200 OK"),
            "a split request head must still yield an ICY greeting, got: {:?}",
            &received[..received.len().min(32)]
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

    fn make_capped_pcm_output(port: u16, max_clients: usize) -> HttpdOutput {
        let mut output = make_pcm_output(port);
        output.max_clients = max_clients;
        output
    }

    /// A connection beyond `max_clients` must be refused rather than queued
    /// indefinitely (SEC-07: unbounded clients exhaust the daemon's fds).
    #[test]
    fn connections_beyond_max_clients_are_refused() {
        let mut output = make_capped_pcm_output(0, 1);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut first = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        first.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        wait_for_clients(&output, 1);

        // Second connection exceeds the cap of 1 and must be closed by the
        // server without a greeting.
        let mut second = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        second
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut buf = [0u8; 16];
        let n = second.read(&mut buf).unwrap_or(0);
        assert_eq!(
            n, 0,
            "connection beyond max_clients must be closed without a greeting"
        );
        assert_eq!(
            output.clients.lock().len(),
            1,
            "refused connection must not be registered as a client"
        );

        output.stop().unwrap();
    }

    /// A client that never drains its socket must be dropped once its bounded
    /// queue fills, instead of ever blocking the caller of `write` (the
    /// realtime audio thread) (SEC-07).
    #[test]
    fn stalled_client_is_dropped_without_blocking_write() {
        let mut output = make_pcm_output(0);
        output.start().expect("start failed");
        let port = output.local_addr().unwrap().port();
        thread::sleep(Duration::from_millis(30));

        let mut client = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        client.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        wait_for_clients(&output, 1);
        // Never read from `client` again: its socket receive buffer plus the
        // server's bounded queue (CLIENT_QUEUE_DEPTH) will fill.

        let chunk = vec![0.5_f32; 16384];
        let start = Instant::now();
        for _ in 0..(CLIENT_QUEUE_DEPTH * 4) {
            output.write(&chunk).expect("write must not fail");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "write must not block on a stalled client (took {elapsed:?})"
        );

        // Close the client: the writer thread hits EPIPE on its next send and
        // must prune the entry (socket-buffer size makes "stall" itself
        // non-deterministic across hosts, so exercise the dead-write path).
        drop(client);
        for _ in 0..4 {
            output.write(&chunk).unwrap();
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            output.clients.lock().len(),
            0,
            "stalled client must be pruned from the client list"
        );

        output.stop().unwrap();
    }
}
