// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Icecast2 source-client output (`type = "shout"`).
//!
//! Connects to an Icecast2 server with an HTTP/1.1 `PUT` request (the modern
//! source protocol, `Expect: 100-continue`), then pushes the encoded audio
//! stream over the same TCP connection.  Pure `std::net`; plain HTTP only
//! (no TLS).  On song change a `GET /admin/metadata?mode=updinfo` request is
//! sent from a short-lived thread so the audio path never blocks on it.
//!
//! Settings: `host` (default `localhost`), `port` (default `8000`), `mount`
//! (default `/rmpd`), `password` (required), `user` (default `source`),
//! `name`, `genre`, `description`, `public` (default `false`), `encoder`
//! (default `flac`; see [`crate::encoder`]), `bitrate`/`quality`/`compression`
//! (forwarded to the encoder) and `sync` (default `true`: pace writes to real
//! time).  The source password is never logged.
//!
//! Note: in-band metadata for Ogg streams is not updated by `/admin/metadata`
//! on every Icecast version; MP3/AAC-style mounts honour it.

use crate::audio_output::{AudioOutput, PauseState};
use crate::encoder::{Encoder, create_encoder};
use crate::httpd_output::now_playing;
use crate::null_output::Pacer;
use rmpd_core::config::OutputConfig;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const USER_AGENT: &str = concat!("rmpd/", env!("CARGO_PKG_VERSION"));

// ── helpers ──────────────────────────────────────────────────────────────────

/// Standard (padded) base64, enough for HTTP Basic credentials.
fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Percent-encode everything except RFC 3986 unreserved characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Strip control characters so a value can never inject extra header lines.
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn parse_status(response: &str) -> Option<u16> {
    response
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

// ── configuration ────────────────────────────────────────────────────────────

/// Parsed `[[output]] type = "shout"` settings.
#[derive(Clone)]
struct ShoutConfig {
    host: String,
    port: u16,
    mount: String,
    user: String,
    password: String,
    name: String,
    genre: String,
    description: String,
    public: bool,
    encoder: String,
    bitrate: Option<String>,
}

impl ShoutConfig {
    fn from_output_config(cfg: &OutputConfig) -> Result<Self> {
        let password = cfg.setting_str("password").ok_or_else(|| {
            RmpdError::Player("shout output requires a 'password' setting".into())
        })?;
        let mut mount = cfg.setting_str("mount").unwrap_or_else(|| "/rmpd".into());
        if !mount.starts_with('/') {
            mount.insert(0, '/');
        }
        let port = match cfg.setting_str("port") {
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| RmpdError::Player(format!("shout output: invalid port '{p}'")))?,
            None => 8000,
        };
        Ok(Self {
            host: cfg
                .setting_str("host")
                .unwrap_or_else(|| "localhost".into()),
            port,
            mount,
            user: cfg.setting_str("user").unwrap_or_else(|| "source".into()),
            password,
            name: cfg.setting_str("name").unwrap_or_else(|| {
                if cfg.name.is_empty() {
                    "rmpd".into()
                } else {
                    cfg.name.clone()
                }
            }),
            genre: cfg.setting_str("genre").unwrap_or_default(),
            description: cfg.setting_str("description").unwrap_or_default(),
            public: cfg.setting_str("public").is_some_and(|v| parse_bool(&v)),
            encoder: crate::encoder::encoder_name_or(cfg, "flac"),
            bitrate: cfg.setting_str("bitrate"),
        })
    }

    fn auth_header(&self) -> String {
        let creds = format!("{}:{}", self.user, self.password);
        format!(
            "Authorization: Basic {}\r\n",
            base64_encode(creds.as_bytes())
        )
    }

    /// The `PUT` request that turns the connection into an Icecast source.
    fn source_request(&self, content_type: &str, format: AudioFormat) -> String {
        let mut audio_info = format!(
            "ice-samplerate={};ice-channels={}",
            format.sample_rate, format.channels
        );
        if let Some(b) = self.bitrate.as_deref().and_then(|b| b.parse::<u32>().ok()) {
            audio_info.push_str(&format!(";ice-bitrate={b}"));
        }
        format!(
            "PUT {mount} HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             {auth}\
             User-Agent: {ua}\r\n\
             Content-Type: {ct}\r\n\
             Ice-Name: {name}\r\n\
             Ice-Public: {public}\r\n\
             Ice-Genre: {genre}\r\n\
             Ice-Description: {desc}\r\n\
             Ice-Audio-Info: {info}\r\n\
             Expect: 100-continue\r\n\
             \r\n",
            mount = sanitize(&self.mount).replace(' ', "%20"),
            host = sanitize(&self.host),
            port = self.port,
            auth = self.auth_header(),
            ua = USER_AGENT,
            ct = sanitize(content_type),
            name = sanitize(&self.name),
            public = u8::from(self.public),
            genre = sanitize(&self.genre),
            desc = sanitize(&self.description),
            info = audio_info,
        )
    }

    /// The `/admin/metadata` request announcing a new "now playing" title.
    fn metadata_request(&self, title: &str) -> String {
        format!(
            "GET /admin/metadata?mode=updinfo&mount={mount}&song={song} HTTP/1.0\r\n\
             Host: {host}:{port}\r\n\
             {auth}\
             User-Agent: {ua}\r\n\
             Connection: close\r\n\
             \r\n",
            mount = percent_encode(&self.mount),
            song = percent_encode(title),
            host = sanitize(&self.host),
            port = self.port,
            auth = self.auth_header(),
            ua = USER_AGENT,
        )
    }

    fn open(&self) -> std::io::Result<TcpStream> {
        let mut last = None;
        for addr in (self.host.as_str(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    s.set_write_timeout(Some(IO_TIMEOUT))?;
                    s.set_read_timeout(Some(IO_TIMEOUT))?;
                    return Ok(s);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("host resolved to no addresses")))
    }

    /// Fire-and-forget metadata update on its own thread.
    fn send_metadata(&self, title: String) {
        let cfg = self.clone();
        thread::spawn(move || {
            let result = (|| -> std::io::Result<u16> {
                let mut s = cfg.open()?;
                s.write_all(cfg.metadata_request(&title).as_bytes())?;
                let mut buf = [0u8; 256];
                let n = s.read(&mut buf)?;
                Ok(parse_status(&String::from_utf8_lossy(&buf[..n])).unwrap_or(0))
            })();
            match result {
                Ok(200) => debug!("shout: metadata updated"),
                Ok(code) => warn!("shout: metadata update rejected (HTTP {code})"),
                Err(e) => warn!("shout: metadata update failed: {e}"),
            }
        });
    }
}

// ── output ───────────────────────────────────────────────────────────────────

pub struct ShoutOutput {
    cfg: ShoutConfig,
    format: AudioFormat,
    encoder: Box<dyn Encoder>,
    stream: Option<TcpStream>,
    next_retry: Instant,
    /// Title last announced via `/admin/metadata` on the current connection.
    meta_sent: Option<String>,
    pacer: Option<Pacer>,
    pause_state: PauseState,
}

impl ShoutOutput {
    /// # Errors
    /// Missing `password`, invalid `port`, or an unknown/invalid encoder.
    pub fn try_new(format: AudioFormat, cfg: &OutputConfig) -> Result<Self> {
        let shout = ShoutConfig::from_output_config(cfg)?;
        let encoder = create_encoder(&shout.encoder, format, cfg)?;
        let sync = cfg.setting_str("sync").is_none_or(|v| parse_bool(&v));
        Ok(Self {
            cfg: shout,
            format,
            encoder,
            stream: None,
            next_retry: Instant::now(),
            meta_sent: None,
            pacer: sync.then(|| Pacer::new(format)),
            pause_state: PauseState::new(),
        })
    }

    fn connect(&mut self) -> std::result::Result<(), String> {
        let mut s = self
            .cfg
            .open()
            .map_err(|e| format!("connect failed: {e}"))?;
        let request = self
            .cfg
            .source_request(self.encoder.content_type(), self.format);
        s.write_all(request.as_bytes())
            .map_err(|e| format!("request failed: {e}"))?;

        // Wait for the status line (`100 Continue` or, on older servers, `200 OK`).
        let mut buf = Vec::with_capacity(256);
        let mut tmp = [0u8; 256];
        while !buf.windows(2).any(|w| w == b"\r\n") && buf.len() < 2048 {
            let n = s.read(&mut tmp).map_err(|e| format!("no response: {e}"))?;
            if n == 0 {
                return Err("server closed the connection".into());
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        match parse_status(&String::from_utf8_lossy(&buf)) {
            Some(100 | 200) => {}
            Some(401) => return Err("authentication failed (HTTP 401)".into()),
            Some(code) => return Err(format!("server rejected the source (HTTP {code})")),
            None => return Err("malformed response".into()),
        }

        let header = self.encoder.header();
        if !header.is_empty() {
            s.write_all(&header)
                .map_err(|e| format!("header write failed: {e}"))?;
        }
        s.set_read_timeout(None).ok();
        self.stream = Some(s);
        self.meta_sent = None;
        info!(
            "shout: streaming to {}:{}{}",
            self.cfg.host, self.cfg.port, self.cfg.mount
        );
        Ok(())
    }

    fn disconnect(&mut self) {
        self.stream = None;
        self.next_retry = Instant::now() + RETRY_INTERVAL;
    }
}

impl AudioOutput for ShoutOutput {
    fn start(&mut self) -> Result<()> {
        if let Err(e) = self.connect() {
            warn!("shout: {e}; will retry");
            self.next_retry = Instant::now() + RETRY_INTERVAL;
        }
        self.pause_state.set_paused(false);
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        if self.is_paused() {
            return Ok(());
        }
        if self.stream.is_none() && Instant::now() >= self.next_retry {
            if let Err(e) = self.connect() {
                warn!("shout: {e}; will retry");
                self.next_retry = Instant::now() + RETRY_INTERVAL;
            }
        }
        if self.stream.is_some() {
            let title = now_playing();
            if title != self.meta_sent {
                if let Some(t) = &title {
                    self.cfg.send_metadata(t.clone());
                }
                self.meta_sent = title;
            }
            let bytes = self.encoder.encode(samples);
            let failed = match self.stream.as_mut() {
                Some(s) => s.write_all(&bytes).is_err(),
                None => false,
            };
            if failed {
                warn!("shout: connection lost; will retry");
                self.disconnect();
            }
        }
        if let Some(p) = self.pacer.as_mut() {
            p.add(samples.len());
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.stream = None;
        if let Some(p) = self.pacer.as_mut() {
            p.reset();
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.pause_state.set_paused(true);
        if let Some(p) = self.pacer.as_mut() {
            p.pause();
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.pause_state.set_paused(false);
        if let Some(p) = self.pacer.as_mut() {
            p.resume();
        }
        Ok(())
    }

    fn pause_state(&self) -> &PauseState {
        &self.pause_state
    }

    fn pause_state_mut(&mut self) -> &mut PauseState {
        &mut self.pause_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn fmt() -> AudioFormat {
        AudioFormat {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        }
    }

    fn cfg(pairs: &[(&str, &str)]) -> OutputConfig {
        let mut settings = toml::Table::new();
        for (k, v) in pairs {
            settings.insert((*k).into(), toml::Value::String((*v).into()));
        }
        OutputConfig {
            name: "Radio".into(),
            output_type: "shout".into(),
            enabled: true,
            settings,
        }
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("Artist - Title"), "Artist%20-%20Title");
        assert_eq!(percent_encode("/mount"), "%2Fmount");
        assert_eq!(percent_encode("ü"), "%C3%BC");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn status_parsing() {
        assert_eq!(parse_status("HTTP/1.1 100 Continue\r\n\r\n"), Some(100));
        assert_eq!(parse_status("HTTP/1.0 401 Unauthorized\r\n"), Some(401));
        assert_eq!(parse_status("garbage"), None);
    }

    #[test]
    fn password_is_required() {
        assert!(ShoutConfig::from_output_config(&cfg(&[])).is_err());
    }

    #[test]
    fn defaults_and_mount_normalisation() {
        let c = ShoutConfig::from_output_config(&cfg(&[("password", "pw"), ("mount", "live")]))
            .unwrap();
        assert_eq!(c.host, "localhost");
        assert_eq!(c.port, 8000);
        assert_eq!(c.mount, "/live");
        assert_eq!(c.user, "source");
        assert_eq!(c.encoder, "flac");
        assert_eq!(c.name, "Radio");
        assert!(!c.public);
    }

    #[test]
    fn source_request_format() {
        let c = ShoutConfig::from_output_config(&cfg(&[
            ("password", "hackme"),
            ("mount", "/stream.ogg"),
            ("host", "ice.example.org"),
            ("port", "8010"),
            ("genre", "Jazz"),
            ("description", "Late night"),
            ("public", "true"),
            ("bitrate", "128"),
        ]))
        .unwrap();
        let req = c.source_request("audio/ogg", fmt());
        assert!(req.starts_with("PUT /stream.ogg HTTP/1.1\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
        assert!(req.contains("Host: ice.example.org:8010\r\n"));
        // base64("source:hackme")
        let expected = format!(
            "Authorization: Basic {}\r\n",
            base64_encode(b"source:hackme")
        );
        assert!(req.contains(&expected));
        assert!(req.contains("Content-Type: audio/ogg\r\n"));
        assert!(req.contains("Ice-Name: Radio\r\n"));
        assert!(req.contains("Ice-Public: 1\r\n"));
        assert!(req.contains("Ice-Genre: Jazz\r\n"));
        assert!(req.contains("Ice-Description: Late night\r\n"));
        assert!(
            req.contains("Ice-Audio-Info: ice-samplerate=44100;ice-channels=2;ice-bitrate=128\r\n")
        );
        assert!(req.contains("Expect: 100-continue\r\n"));
    }

    #[test]
    fn header_values_cannot_inject_lines() {
        let c = ShoutConfig::from_output_config(&cfg(&[
            ("password", "pw"),
            ("name", "evil\r\nX-Injected: 1"),
        ]))
        .unwrap();
        let req = c.source_request("audio/ogg", fmt());
        assert!(!req.contains("\r\nX-Injected"));
    }

    #[test]
    fn metadata_request_format() {
        let c =
            ShoutConfig::from_output_config(&cfg(&[("password", "pw"), ("mount", "/m")])).unwrap();
        let req = c.metadata_request("Björk - Jóga");
        assert!(req.starts_with(
            "GET /admin/metadata?mode=updinfo&mount=%2Fm&song=Bj%C3%B6rk%20-%20J%C3%B3ga HTTP/1.0\r\n"
        ));
        assert!(req.contains("Authorization: Basic "));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn unknown_encoder_is_rejected() {
        let c = cfg(&[("password", "pw"), ("encoder", "nope")]);
        assert!(ShoutOutput::try_new(fmt(), &c).is_err());
    }

    #[test]
    fn streams_to_a_fake_icecast() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut req = Vec::new();
            let mut tmp = [0u8; 512];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut tmp).unwrap();
                assert!(n > 0);
                req.extend_from_slice(&tmp[..n]);
            }
            s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").unwrap();
            let mut body = Vec::new();
            while body.len() < 40 {
                match s.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&tmp[..n]),
                }
            }
            (String::from_utf8_lossy(&req).into_owned(), body)
        });

        let c = cfg(&[
            ("password", "pw"),
            ("host", "127.0.0.1"),
            ("port", port.to_string().as_str()),
            ("mount", "/t"),
            ("encoder", "pcm"),
        ]);
        let mut out = ShoutOutput::try_new(fmt(), &c).unwrap();
        out.start().unwrap();
        assert!(out.stream.is_some(), "handshake should succeed");
        out.write(&[0.0f32; 20]).unwrap();
        out.stop().unwrap();

        let (req, body) = server.join().unwrap();
        assert!(req.starts_with("PUT /t HTTP/1.1\r\n"));
        assert_eq!(body.len(), 40, "20 samples -> 40 PCM bytes");
    }

    #[test]
    fn rejected_credentials_leave_output_disconnected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut tmp = [0u8; 512];
            let _ = s.read(&mut tmp);
            s.write_all(b"HTTP/1.1 401 Unauthorized\r\n\r\n").unwrap();
        });
        let c = cfg(&[
            ("password", "bad"),
            ("host", "127.0.0.1"),
            ("port", port.to_string().as_str()),
            ("encoder", "pcm"),
        ]);
        let mut out = ShoutOutput::try_new(fmt(), &c).unwrap();
        out.start().unwrap(); // retries later, never errors
        assert!(out.stream.is_none());
        server.join().unwrap();
    }
}
