//! HTTP/streaming input source for rmpd (internet radio).
//!
//! Provides [`HttpSource`], a Symphonia [`MediaSource`] that streams audio over
//! HTTP(S) using a blocking reqwest client, with optional Shoutcast/Icecast
//! (ICY) metadata de-interleaving. Metadata blocks are stripped so the bytes
//! handed to the decoder are pure audio, and the "now playing" title is
//! surfaced through a cheap shared handle ([`TitleHandle`]).
#![allow(clippy::cargo_common_metadata)]

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use symphonia::core::io::MediaSource;

/// Shared, thread-safe handle to the latest ICY "now playing" title.
pub type TitleHandle = Arc<Mutex<Option<String>>>;

/// Whether `uri` looks like a remote stream this crate can open.
#[must_use]
pub fn is_http_uri(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// A streaming HTTP(S) media source for Symphonia.
///
/// The inner reader is wrapped in a [`Mutex`] purely so the whole struct is
/// `Sync` (required by [`MediaSource`]); reads are still serialized through the
/// single decoder thread.
pub struct HttpSource {
    inner: Mutex<Box<dyn Read + Send>>,
    /// ICY metadata interval (audio bytes between metadata blocks), if any.
    metaint: Option<usize>,
    /// Audio bytes remaining before the next ICY metadata block.
    bytes_until_meta: usize,
    /// Latest parsed "now playing" title.
    title: TitleHandle,
}

impl HttpSource {
    /// Connect to `url` and begin streaming. Requests ICY metadata; when the
    /// server advertises `icy-metaint`, metadata blocks are de-interleaved out
    /// of the audio stream and the title handle is updated as they arrive.
    ///
    /// # Errors
    /// Returns an error if the client cannot be built, the request fails, or
    /// the server responds with a non-success status.
    pub fn connect(url: &str) -> io::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(None) // streams are open-ended; no overall deadline
            .connect_timeout(Duration::from_secs(15))
            .user_agent("rmpd")
            .build()
            .map_err(to_io)?;
        let resp = client
            .get(url)
            .header("Icy-MetaData", "1")
            .send()
            .map_err(to_io)?
            .error_for_status()
            .map_err(to_io)?;
        let metaint = resp
            .headers()
            .get("icy-metaint")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n > 0);
        tracing::debug!(url, ?metaint, "opened HTTP stream");
        Ok(Self::with_reader(Box::new(resp), metaint))
    }

    /// Build a source from an arbitrary byte reader. `metaint` mirrors the
    /// `icy-metaint` header (None disables ICY de-interleaving). Used for tests
    /// and alternative transports.
    #[must_use]
    pub fn with_reader(reader: Box<dyn Read + Send>, metaint: Option<usize>) -> Self {
        Self {
            inner: Mutex::new(reader),
            metaint,
            bytes_until_meta: metaint.unwrap_or(0),
            title: Arc::new(Mutex::new(None)),
        }
    }

    /// A shared handle to the latest ICY "now playing" title.
    #[must_use]
    pub fn title_handle(&self) -> TitleHandle {
        Arc::clone(&self.title)
    }
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut inner = self.inner.lock();
        let Some(mi) = self.metaint else {
            return inner.read(buf);
        };

        if self.bytes_until_meta == 0 {
            // At a metadata boundary: read the length byte. A clean EOF here
            // simply ends the stream (radio streams are otherwise endless).
            let mut len_byte = [0u8; 1];
            if inner.read(&mut len_byte)? == 0 {
                return Ok(0);
            }
            let len = len_byte[0] as usize * 16;
            if len > 0 {
                let mut block = vec![0u8; len];
                read_exact_eof(&mut *inner, &mut block)?;
                if let Some(t) = parse_stream_title(&block) {
                    *self.title.lock() = Some(t);
                }
            }
            self.bytes_until_meta = mi;
        }

        let want = buf.len().min(self.bytes_until_meta);
        let n = inner.read(&mut buf[..want])?;
        self.bytes_until_meta -= n;
        Ok(n)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        // Streams are not seekable; tolerate only the "tell" idiom.
        if matches!(pos, SeekFrom::Current(0)) {
            return Ok(0);
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HTTP stream is not seekable",
        ))
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn to_io(e: reqwest::Error) -> io::Error {
    io::Error::other(e.to_string())
}

/// Like `read_exact`, but maps an early EOF to `UnexpectedEof` so a truncated
/// metadata block ends the stream cleanly instead of spinning.
fn read_exact_eof(reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended mid-metadata",
            ));
        }
        filled += n;
    }
    Ok(())
}

/// Parse `StreamTitle='...';` out of an ICY metadata block. Returns `None` for
/// missing or empty titles.
#[must_use]
pub fn parse_stream_title(block: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(block);
    let start = text.find("StreamTitle=")? + "StreamTitle=".len();
    let rest = text[start..].strip_prefix('\'')?;
    let end = rest.find("';").or_else(|| rest.find('\''))?;
    let title = rest[..end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build an ICY metadata block (length byte + zero-padded payload).
    fn meta_block(title: &str) -> Vec<u8> {
        let s = format!("StreamTitle='{title}';");
        let blocks = s.len().div_ceil(16);
        let mut v = Vec::with_capacity(1 + blocks * 16);
        v.push(blocks as u8);
        v.extend_from_slice(s.as_bytes());
        v.resize(1 + blocks * 16, 0);
        v
    }

    #[test]
    fn deinterleaves_and_parses_title() {
        let mut raw: Vec<u8> = (0u8..16).collect(); // 16 audio bytes
        raw.extend_from_slice(&meta_block("Artist - Song")); // metadata block
        raw.extend((16u8..32).collect::<Vec<_>>()); // 16 audio bytes
        raw.push(0); // zero-length metadata (no title)
        raw.extend_from_slice(&[99, 98, 97]); // trailing audio bytes

        let mut src = HttpSource::with_reader(Box::new(Cursor::new(raw)), Some(16));
        let title = src.title_handle();

        // Read in small chunks to exercise partial reads across boundaries.
        let mut out = Vec::new();
        let mut buf = [0u8; 7];
        loop {
            match src.read(&mut buf).unwrap() {
                0 => break,
                n => out.extend_from_slice(&buf[..n]),
            }
        }

        let mut expected: Vec<u8> = (0u8..32).collect();
        expected.extend_from_slice(&[99, 98, 97]);
        assert_eq!(out, expected, "metadata bytes must be stripped from audio");
        assert_eq!(title.lock().as_deref(), Some("Artist - Song"));
    }

    #[test]
    fn no_metaint_is_passthrough() {
        let raw: Vec<u8> = (0u8..50).collect();
        let mut src = HttpSource::with_reader(Box::new(Cursor::new(raw.clone())), None);
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn parse_title_variants() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Hello World';StreamUrl='http://x';").as_deref(),
            Some("Hello World")
        );
        assert_eq!(parse_stream_title(b"StreamTitle='';"), None);
        assert_eq!(parse_stream_title(b"NoTitleHere"), None);
        // Tolerate a missing trailing terminator.
        assert_eq!(
            parse_stream_title(b"StreamTitle='A - B'").as_deref(),
            Some("A - B")
        );
    }

    #[test]
    fn empty_metadata_keeps_title_none() {
        // metaint with only zero-length metadata blocks => no title surfaces.
        let mut raw: Vec<u8> = (0u8..8).collect();
        raw.push(0); // zero-length metadata
        raw.extend_from_slice(&[1, 2, 3]);
        let mut src = HttpSource::with_reader(Box::new(Cursor::new(raw)), Some(8));
        let title = src.title_handle();
        let mut out = Vec::new();
        src.read_to_end(&mut out).unwrap();
        assert_eq!(out, vec![0, 1, 2, 3, 4, 5, 6, 7, 1, 2, 3]);
        assert!(title.lock().is_none());
    }

    #[test]
    fn not_seekable() {
        let src = HttpSource::with_reader(Box::new(Cursor::new(vec![1, 2, 3])), None);
        assert!(!src.is_seekable());
        assert_eq!(src.byte_len(), None);
    }
}
