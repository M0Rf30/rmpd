// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HLS (HTTP Live Streaming, RFC 8216) input.
//!
//! [`open_hls`] turns an HLS playlist URL into a blocking, non-seekable
//! [`MediaSource`]:
//!
//! 1. A master playlist is reduced to one variant (audio-only preferred,
//!    optionally capped by `[stream] hls_max_bandwidth`).
//! 2. A background thread walks the media playlist (reloading live playlists
//!    at the target duration), downloads segments, decrypts `AES-128`
//!    segments (pure-Rust RustCrypto), and turns them into a raw byte stream:
//!    ADTS-AAC and MP3 segments pass through (ID3 tags stripped), fMP4
//!    segments follow their `EXT-X-MAP` init section, and MPEG-TS segments are
//!    demultiplexed into the elementary audio stream.
//! 3. The decoder reads that stream through [`HlsSource`]; segments are
//!    prefetched a couple of items ahead through a bounded channel.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use aes::Aes128;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use parking_lot::Mutex;
use reqwest::blocking::Client;
use rmpd_core::config::StreamConfig;
use symphonia::core::io::MediaSource;

use crate::hls_playlist::{
    self as playlist, ByteRange, KeyInfo, KeyMethod, MapInfo, MediaPlaylist, Playlist, Segment,
};
use crate::input::OpenedInput;
use crate::radio_playlist::{MAX_PLAYLIST_BYTES, read_capped};
use crate::ts::{AudioKind, TsDemuxer, looks_like_ts};
use crate::{redact_url, to_io};

/// Largest single segment (or init section) that will be buffered.
const SEGMENT_CAP: u64 = 64 * 1024 * 1024;
/// Largest AES key response.
const KEY_CAP: u64 = 4096;
/// Segments downloaded ahead of the decoder.
const PREFETCH: usize = 2;
/// A live stream starts this many segments before the end of the playlist.
const LIVE_EDGE_SEGMENTS: usize = 3;
/// Download attempts per resource.
const FETCH_ATTEMPTS: u32 = 3;
/// Consecutive failed live playlist reloads tolerated.
const MAX_RELOAD_ERRORS: u32 = 3;
/// Granularity at which sleeping workers notice a stop request.
const SLEEP_SLICE: Duration = Duration::from_millis(100);

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

// ---------------------------------------------------------------------------
// Container sniffing and segment post-processing (pure).
// ---------------------------------------------------------------------------

/// Container format of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    /// MPEG transport stream.
    Ts,
    /// (Fragmented) ISO base media file.
    Mp4,
    /// Raw ADTS AAC.
    Adts,
    /// Raw MPEG audio (MP3/MP2).
    Mpeg,
    /// Unrecognized: passed through untouched.
    Unknown,
}

/// Length of the ID3v2 tag(s) at the start of `data` (0 when none).
fn id3_len(data: &[u8]) -> usize {
    let mut total = 0;
    while let Some(rest) = data.get(total..)
        && rest.len() >= 10
        && rest.starts_with(b"ID3")
        && rest[6..10].iter().all(|b| b & 0x80 == 0)
    {
        let size = rest[6..10]
            .iter()
            .fold(0usize, |acc, b| (acc << 7) | usize::from(*b));
        let footer = if rest[5] & 0x10 != 0 { 10 } else { 0 };
        let tag = 10 + size + footer;
        if tag > rest.len() {
            // Truncated tag: nothing playable follows.
            return data.len();
        }
        total += tag;
    }
    total
}

/// Identify the container of `data` (ID3 already stripped).
fn sniff(data: &[u8]) -> Container {
    if looks_like_ts(data) {
        return Container::Ts;
    }
    if data.len() >= 8 && matches!(&data[4..8], b"ftyp" | b"styp" | b"moof" | b"moov" | b"sidx") {
        return Container::Mp4;
    }
    if data.len() >= 2 && data[0] == 0xFF {
        let b = data[1];
        if b & 0xF6 == 0xF0 {
            return Container::Adts;
        }
        // MPEG audio frame sync, valid version and layer.
        if b & 0xE0 == 0xE0 && (b >> 3) & 0x3 != 1 && (b >> 1) & 0x3 != 0 {
            return Container::Mpeg;
        }
    }
    Container::Unknown
}

/// Turns downloaded segment bytes into the byte stream handed to Symphonia.
#[derive(Debug, Default)]
struct SegmentProcessor {
    container: Option<Container>,
    ts: TsDemuxer,
}

impl SegmentProcessor {
    /// Process one (already decrypted) segment. `has_map`: the segment is
    /// fMP4 media following an `EXT-X-MAP` init section.
    fn process(&mut self, mut data: Vec<u8>, has_map: bool) -> io::Result<Vec<u8>> {
        let skip = id3_len(&data);
        if skip > 0 {
            data.drain(..skip);
        }
        let found = if has_map {
            Container::Mp4
        } else {
            sniff(&data)
        };
        let container = match (found, self.container) {
            (Container::Unknown, Some(known)) => known,
            (found, _) => found,
        };
        if self.container.is_none() {
            self.container = Some(container);
        }
        match container {
            Container::Ts => {
                let mut es = self.ts.push(&data);
                es.extend(self.ts.finish());
                if es.is_empty() && self.ts.kind().is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "MPEG-TS segment has no AAC or MP3 audio stream",
                    ));
                }
                Ok(es)
            }
            Container::Mp4 | Container::Adts | Container::Mpeg | Container::Unknown => Ok(data),
        }
    }

    /// A segment boundary that interrupts timing/continuity.
    fn discontinuity(&mut self) {
        self.ts.reset();
    }

    /// Demuxer probe hint matching the stream produced so far.
    fn extension_hint(&self) -> Option<&'static str> {
        match self.container? {
            Container::Mp4 => Some("mp4"),
            Container::Adts => Some("aac"),
            Container::Mpeg => Some("mp3"),
            Container::Ts => match self.ts.kind()? {
                AudioKind::AdtsAac => Some("aac"),
                AudioKind::Mpeg => Some("mp3"),
            },
            Container::Unknown => None,
        }
    }
}

// ---------------------------------------------------------------------------
// AES-128-CBC.
// ---------------------------------------------------------------------------

/// Decrypt an `AES-128` segment: CBC with PKCS#7 padding.
fn aes128_cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> io::Result<Vec<u8>> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(invalid(
            "encrypted HLS segment is not a multiple of 16 bytes",
        ));
    }
    let cipher = cbc::Decryptor::<Aes128>::new_from_slices(key, iv)
        .map_err(|_| invalid("invalid AES-128 key or IV length"))?;
    let mut buf = data.to_vec();
    let len = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| invalid("HLS segment decryption failed (bad key or padding)"))?
        .len();
    buf.truncate(len);
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Playlist window tracking (pure).
// ---------------------------------------------------------------------------

/// Tracks which segment of a (possibly sliding) playlist is delivered next.
#[derive(Debug)]
struct Cursor {
    playlist: MediaPlaylist,
    next_seq: u64,
}

impl Cursor {
    /// Start at the segment with the given index in `playlist`.
    fn new(playlist: MediaPlaylist, start_index: usize) -> Self {
        let next_seq = playlist.media_sequence + start_index as u64;
        Self { playlist, next_seq }
    }

    /// Sequence number just past the newest segment.
    fn end(&self) -> u64 {
        self.playlist.media_sequence + self.playlist.segments.len() as u64
    }

    /// The next segment, if the playlist already lists it. Falls forward when
    /// the playlist window slid past segments we never fetched.
    fn take(&mut self) -> Option<Segment> {
        let first = self.playlist.media_sequence;
        if self.next_seq < first {
            tracing::warn!(
                skipped = first - self.next_seq,
                "HLS playlist moved past unfetched segments"
            );
            self.next_seq = first;
        }
        let idx = usize::try_from(self.next_seq - first).ok()?;
        let seg = self.playlist.segments.get(idx)?.clone();
        self.next_seq += 1;
        Some(seg)
    }

    /// Install a reloaded playlist; returns whether it lists new segments.
    fn update(&mut self, new: MediaPlaylist) -> bool {
        if new.media_sequence + (new.segments.len() as u64) < self.next_seq {
            // The origin restarted its numbering: rejoin the live edge.
            self.next_seq =
                new.media_sequence + new.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS) as u64;
        }
        self.playlist = new;
        self.end() > self.next_seq
    }
}

// ---------------------------------------------------------------------------
// Networking helpers.
// ---------------------------------------------------------------------------

fn http_get(
    client: &Client,
    url: &str,
    range: Option<ByteRange>,
) -> io::Result<reqwest::blocking::Response> {
    let mut req = client.get(url);
    if let Some(r) = range {
        req = req.header(reqwest::header::RANGE, r.header_value());
    }
    req.send().map_err(to_io)?.error_for_status().map_err(to_io)
}

/// Download `url` (optionally a byte range), failing above `cap` bytes.
fn fetch_bytes(
    client: &Client,
    url: &str,
    range: Option<ByteRange>,
    cap: u64,
) -> io::Result<Vec<u8>> {
    let mut resp = http_get(client, url, range)?;
    if let Some(r) = range
        && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT
    {
        // The server ignored the Range header and sent the whole resource.
        io::copy(&mut resp.by_ref().take(r.offset), &mut io::sink())?;
        return read_limited(resp.take(r.len), cap);
    }
    read_limited(resp, cap)
}

fn read_limited(reader: impl Read, cap: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(invalid(format!("HLS resource larger than {cap} bytes")));
    }
    Ok(buf)
}

fn fetch_text(client: &Client, url: &str) -> io::Result<String> {
    read_capped(http_get(client, url, None)?, MAX_PLAYLIST_BYTES)
}

/// Sleep `dur` in short slices; returns `false` when `stop` was raised.
fn sleep_interruptible(stop: &AtomicBool, dur: Duration) -> bool {
    let deadline = Instant::now() + dur;
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        std::thread::sleep(SLEEP_SLICE.min(deadline - now));
    }
}

// ---------------------------------------------------------------------------
// Segment fetching worker.
// ---------------------------------------------------------------------------

struct Worker {
    client: Client,
    media_url: String,
    cursor: Cursor,
    stop: Arc<AtomicBool>,
    keys: HashMap<String, [u8; 16]>,
    current_map: Option<Arc<MapInfo>>,
    proc: SegmentProcessor,
    last_reload: Instant,
    last_progress: Instant,
    /// Consecutive reloads that brought no new segment.
    empty_reloads: u32,
    reload_errors: u32,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Download with retries (stop-aware).
    fn fetch(&self, url: &str, range: Option<ByteRange>, cap: u64) -> io::Result<Vec<u8>> {
        let mut attempt = 1;
        loop {
            match fetch_bytes(&self.client, url, range, cap) {
                Ok(data) => return Ok(data),
                Err(e) if attempt >= FETCH_ATTEMPTS || self.stopped() => return Err(e),
                Err(e) => {
                    tracing::debug!(
                        url = %redact_url(url),
                        error = %e,
                        attempt,
                        "HLS download failed, retrying"
                    );
                    attempt += 1;
                    if !sleep_interruptible(&self.stop, Duration::from_millis(500)) {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "HLS stopped"));
                    }
                }
            }
        }
    }

    fn decrypt(&mut self, data: &[u8], key: &KeyInfo, sequence: u64) -> io::Result<Vec<u8>> {
        if let KeyMethod::Unsupported(method) = &key.method {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("HLS encryption method {method} is not supported (only AES-128)"),
            ));
        }
        let uri = key
            .uri
            .as_deref()
            .ok_or_else(|| invalid("HLS AES-128 key has no URI"))?;
        let k = if let Some(k) = self.keys.get(uri) {
            *k
        } else {
            let bytes = self.fetch(uri, None, KEY_CAP)?;
            let k: [u8; 16] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| invalid("HLS AES-128 key is not 16 bytes"))?;
            if self.keys.len() >= 16 {
                self.keys.clear();
            }
            self.keys.insert(uri.to_owned(), k);
            k
        };
        aes128_cbc_decrypt(&k, &playlist::segment_iv(key, sequence), data)
    }

    /// Download one resource of `seg` and decrypt it when the segment is.
    fn fetch_decrypted(
        &mut self,
        url: &str,
        range: Option<ByteRange>,
        seg: &Segment,
    ) -> io::Result<Vec<u8>> {
        let data = self.fetch(url, range, SEGMENT_CAP)?;
        match &seg.key {
            Some(key) => self.decrypt(&data, key, seg.sequence),
            None => Ok(data),
        }
    }

    /// Download and convert one segment (init section first when it changed).
    fn process_segment(&mut self, seg: &Segment) -> io::Result<Vec<u8>> {
        if seg.discontinuity {
            self.proc.discontinuity();
        }
        let mut out = Vec::new();
        if let Some(map) = &seg.map
            && self.current_map.as_ref() != Some(map)
        {
            out = self.fetch_decrypted(&map.uri, map.range, seg)?;
            self.current_map = Some(Arc::clone(map));
        }
        let data = self.fetch_decrypted(&seg.uri, seg.range, seg)?;
        out.extend(self.proc.process(data, seg.map.is_some())?);
        Ok(out)
    }

    /// Wait for and apply a live playlist reload.
    fn reload(&mut self) -> io::Result<()> {
        let target = self.cursor.playlist.target_duration.max(1);
        if self.last_progress.elapsed() > Duration::from_secs(target.max(5) * 3) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HLS playlist stopped advancing",
            ));
        }
        // One target duration after a useful reload, half of it while the
        // origin has nothing new (RFC 8216 §6.3.4).
        let interval = if self.empty_reloads == 0 {
            Duration::from_secs(target)
        } else {
            Duration::from_millis(target * 500)
        };
        let due = self.last_reload + interval;
        let now = Instant::now();
        if due > now && !sleep_interruptible(&self.stop, due - now) {
            return Ok(());
        }
        self.last_reload = Instant::now();
        let fetched = fetch_text(&self.client, &self.media_url)
            .and_then(|text| parse_media(&text, &self.media_url));
        match fetched {
            Ok(pl) => {
                self.reload_errors = 0;
                if self.cursor.update(pl) {
                    self.empty_reloads = 0;
                } else {
                    self.empty_reloads += 1;
                }
            }
            Err(e) => {
                self.reload_errors += 1;
                self.empty_reloads += 1;
                tracing::warn!(
                    url = %redact_url(&self.media_url),
                    error = %e,
                    "HLS playlist reload failed"
                );
                if self.reload_errors >= MAX_RELOAD_ERRORS {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// The next converted segment; `None` at the end of the stream.
    fn next_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if self.stopped() {
                return Ok(None);
            }
            if let Some(seg) = self.cursor.take() {
                self.last_progress = Instant::now();
                tracing::trace!(
                    sequence = seg.sequence,
                    duration = seg.duration,
                    "fetching HLS segment"
                );
                let data = self.process_segment(&seg)?;
                self.last_progress = Instant::now();
                if data.is_empty() {
                    continue;
                }
                return Ok(Some(data));
            }
            if self.cursor.playlist.end_list {
                return Ok(None);
            }
            self.reload()?;
        }
    }

    /// Worker thread body: feed converted segments into the channel.
    fn run(mut self, tx: &SyncSender<io::Result<Vec<u8>>>) {
        loop {
            match self.next_chunk() {
                Ok(Some(chunk)) => {
                    if tx.send(Ok(chunk)).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    if !self.stopped() {
                        tracing::warn!(error = %e, "HLS stream ended with an error");
                        let _ = tx.send(Err(e));
                    }
                    return;
                }
            }
        }
    }
}

fn parse_media(text: &str, url: &str) -> io::Result<MediaPlaylist> {
    match playlist::parse(text, url)? {
        Playlist::Media(m) => Ok(m),
        Playlist::Master(_) => Err(invalid("expected an HLS media playlist")),
    }
}

/// Resolve `body` (fetched from `url`) to a media playlist, selecting and
/// fetching a variant when it is a master playlist. Returns the media
/// playlist URL alongside it.
fn resolve_media(
    client: &Client,
    url: &str,
    body: &str,
    max_bandwidth: Option<u64>,
) -> io::Result<(String, MediaPlaylist)> {
    match playlist::parse(body, url)? {
        Playlist::Media(m) => Ok((url.to_owned(), m)),
        Playlist::Master(master) => {
            let target = playlist::select_variant(&master, max_bandwidth).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "HLS master playlist has no variants",
                )
            })?;
            tracing::debug!(
                variant = %redact_url(&target),
                variants = master.variants.len(),
                ?max_bandwidth,
                "selected HLS variant"
            );
            let text = fetch_text(client, &target)?;
            let media = parse_media(&text, &target)?;
            Ok((target, media))
        }
    }
}

// ---------------------------------------------------------------------------
// The MediaSource handed to the decoder.
// ---------------------------------------------------------------------------

/// Blocking byte stream over an HLS presentation (see the module docs).
pub struct HlsSource {
    rx: Mutex<Receiver<io::Result<Vec<u8>>>>,
    cur: Vec<u8>,
    pos: usize,
    done: bool,
    stop: Arc<AtomicBool>,
}

impl HlsSource {
    fn new(rx: Receiver<io::Result<Vec<u8>>>, first: Vec<u8>, stop: Arc<AtomicBool>) -> Self {
        Self {
            rx: Mutex::new(rx),
            cur: first,
            pos: 0,
            done: false,
            stop,
        }
    }
}

impl Drop for HlsSource {
    fn drop(&mut self) {
        // The worker notices at its next sleep slice / failed channel send.
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Read for HlsSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.pos >= self.cur.len() {
            if self.done {
                return Ok(0);
            }
            match self.rx.get_mut().recv() {
                Ok(Ok(chunk)) => {
                    self.cur = chunk;
                    self.pos = 0;
                }
                Ok(Err(e)) => {
                    self.done = true;
                    return Err(e);
                }
                Err(_) => self.done = true,
            }
        }
        let n = buf.len().min(self.cur.len() - self.pos);
        buf[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Seek for HlsSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if matches!(pos, SeekFrom::Current(0)) {
            return Ok(0);
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HLS stream is not seekable",
        ))
    }
}

impl MediaSource for HlsSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Open the HLS presentation whose playlist `body` was fetched from
/// `playlist_url`. `uri` is the URI the user asked for.
///
/// Fetches the first segment synchronously (to learn the audio format for the
/// probe hint and to surface unsupported content early), then streams the
/// rest from a background thread.
///
/// # Errors
/// Network failures, malformed playlists, unsupported encryption or segment
/// formats.
pub(crate) fn open_hls(
    client: Client,
    playlist_url: &str,
    body: &str,
    cfg: &StreamConfig,
    uri: &str,
) -> io::Result<OpenedInput> {
    let (media_url, media) = resolve_media(&client, playlist_url, body, cfg.hls_max_bandwidth)?;
    if media.segments.is_empty() && media.end_list {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "HLS playlist contains no segments",
        ));
    }
    let start = if media.end_list {
        0
    } else {
        media.segments.len().saturating_sub(LIVE_EDGE_SEGMENTS)
    };
    tracing::debug!(
        url = %redact_url(&media_url),
        segments = media.segments.len(),
        target_duration = media.target_duration,
        live = !media.end_list,
        "opening HLS stream"
    );
    let stop = Arc::new(AtomicBool::new(false));
    let now = Instant::now();
    let mut worker = Worker {
        client,
        media_url,
        cursor: Cursor::new(media, start),
        stop: Arc::clone(&stop),
        keys: HashMap::new(),
        current_map: None,
        proc: SegmentProcessor::default(),
        last_reload: now,
        last_progress: now,
        empty_reloads: 0,
        reload_errors: 0,
    };
    let first = worker.next_chunk()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HLS stream has no playable segments",
        )
    })?;
    let hint = worker.proc.extension_hint();
    let (tx, rx) = sync_channel(PREFETCH);
    std::thread::Builder::new()
        .name("rmpd-hls".to_owned())
        .spawn(move || worker.run(&tx))?;
    Ok(OpenedInput {
        source: Box::new(HlsSource::new(rx, first, stop)),
        title: None,
        extension_hint: hint.map(str::to_owned),
        uri: uri.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;
    use cbc::cipher::block_padding::NoPadding;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn media(text: &str) -> MediaPlaylist {
        parse_media(text, "https://h/p/live.m3u8").expect("media")
    }

    const ADTS: [u8; 8] = [0xFF, 0xF1, 0x50, 0x80, 0x02, 0x1F, 0xFC, 0x00];
    const MP3: [u8; 6] = [0xFF, 0xFB, 0x90, 0x00, 0x00, 0x00];

    fn id3(payload_len: usize) -> Vec<u8> {
        let mut t = b"ID3".to_vec();
        t.extend_from_slice(&[4, 0, 0]);
        t.extend_from_slice(&[
            0,
            0,
            (payload_len >> 7) as u8 & 0x7F,
            payload_len as u8 & 0x7F,
        ]);
        t.extend(std::iter::repeat_n(0x11, payload_len));
        t
    }

    #[test]
    fn id3_tags_are_measured_and_stripped() {
        assert_eq!(id3_len(&ADTS), 0);
        let mut data = id3(300);
        let tag_len = data.len();
        assert_eq!(tag_len, 310);
        data.extend_from_slice(&ADTS);
        assert_eq!(id3_len(&data), tag_len);
        // Back-to-back tags.
        let mut two = id3(5);
        two.extend(id3(7));
        two.extend_from_slice(&MP3);
        assert_eq!(id3_len(&two), 15 + 17);
        // A truncated tag swallows everything.
        let trunc = id3(50)[..30].to_vec();
        assert_eq!(id3_len(&trunc), 30);
    }

    #[test]
    fn sniffing_recognizes_each_container() {
        assert_eq!(sniff(&ADTS), Container::Adts);
        assert_eq!(sniff(&MP3), Container::Mpeg);
        assert_eq!(
            sniff(&[0, 0, 0, 0x20, b'f', b't', b'y', b'p', 0, 0]),
            Container::Mp4
        );
        assert_eq!(
            sniff(&[0, 0, 1, 0, b'm', b'o', b'o', b'f', 0, 0]),
            Container::Mp4
        );
        let mut ts = vec![0u8; 2 * crate::ts::PACKET_SIZE];
        ts[0] = 0x47;
        ts[crate::ts::PACKET_SIZE] = 0x47;
        assert_eq!(sniff(&ts), Container::Ts);
        assert_eq!(sniff(b"hello world"), Container::Unknown);
        assert_eq!(sniff(&[]), Container::Unknown);
    }

    #[test]
    fn adts_segments_pass_through_without_id3() {
        let mut seg = id3(10);
        seg.extend_from_slice(&ADTS);
        let mut p = SegmentProcessor::default();
        assert_eq!(p.process(seg, false).expect("ok"), ADTS);
        assert_eq!(p.extension_hint(), Some("aac"));
    }

    #[test]
    fn mp3_segments_pass_through() {
        let mut p = SegmentProcessor::default();
        assert_eq!(p.process(MP3.to_vec(), false).expect("ok"), MP3);
        assert_eq!(p.extension_hint(), Some("mp3"));
    }

    #[test]
    fn mapped_segments_are_mp4_and_untouched() {
        let frag = vec![0, 0, 0, 8, b'm', b'o', b'o', b'f', 1, 2, 3];
        let mut p = SegmentProcessor::default();
        assert_eq!(p.process(frag.clone(), true).expect("ok"), frag);
        assert_eq!(p.extension_hint(), Some("mp4"));
    }

    #[test]
    fn unknown_later_segment_keeps_known_container() {
        let mut p = SegmentProcessor::default();
        p.process(ADTS.to_vec(), false).expect("ok");
        assert_eq!(p.process(b"junk".to_vec(), false).expect("ok"), b"junk");
        assert_eq!(p.extension_hint(), Some("aac"));
    }

    #[test]
    fn ts_segment_without_supported_audio_is_unsupported() {
        // A single null packet: valid TS framing, no PAT/PMT.
        let mut pkt = vec![0x47, 0x1F, 0xFF, 0x10];
        pkt.resize(crate::ts::PACKET_SIZE, 0xFF);
        let mut two = pkt.clone();
        two.extend(pkt);
        let mut p = SegmentProcessor::default();
        let err = p.process(two, false).expect_err("no audio");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn aes128_cbc_matches_nist_vector() {
        // NIST SP 800-38A F.2.2, CBC-AES128.Decrypt block 1 (no padding).
        let key: [u8; 16] = hex("2b7e151628aed2a6abf7158809cf4f3c")
            .try_into()
            .expect("16");
        let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .expect("16");
        let mut ct = hex("7649abac8119b246cee98e9b12e9197d");
        let plain = cbc::Decryptor::<Aes128>::new_from_slices(&key, &iv)
            .expect("key/iv")
            .decrypt_padded_mut::<NoPadding>(&mut ct)
            .expect("decrypt")
            .to_vec();
        assert_eq!(plain, hex("6bc1bee22e409f96e93d7e117393172a"));
    }

    #[test]
    fn aes128_segment_round_trip_with_pkcs7() {
        let key = [7u8; 16];
        let iv = [9u8; 16];
        let plain: Vec<u8> = (0..100u8).collect();
        let mut buf = vec![0u8; 112];
        buf[..100].copy_from_slice(&plain);
        let n = cbc::Encryptor::<Aes128>::new_from_slices(&key, &iv)
            .expect("key/iv")
            .encrypt_padded_mut::<Pkcs7>(&mut buf, 100)
            .expect("encrypt")
            .len();
        assert_eq!(n, 112);
        assert_eq!(aes128_cbc_decrypt(&key, &iv, &buf).expect("decrypt"), plain);
        // Wrong key: padding check (almost surely) fails or yields garbage.
        let wrong = aes128_cbc_decrypt(&[8u8; 16], &iv, &buf);
        assert!(wrong.is_err() || wrong.expect("garbage") != plain);
        // Not a multiple of the block size.
        assert!(aes128_cbc_decrypt(&key, &iv, &buf[..15]).is_err());
        assert!(aes128_cbc_decrypt(&key, &iv, &[]).is_err());
    }

    const LIVE: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:10\n\
#EXTINF:6,\ns10.ts\n#EXTINF:6,\ns11.ts\n#EXTINF:6,\ns12.ts\n";

    #[test]
    fn cursor_walks_playlist_and_detects_new_segments() {
        let mut c = Cursor::new(media(LIVE), 1);
        assert_eq!(c.take().map(|s| s.sequence), Some(11));
        assert_eq!(c.take().map(|s| s.sequence), Some(12));
        assert!(c.take().is_none());

        // Unchanged playlist: nothing new.
        assert!(!c.update(media(LIVE)));
        // Window slid by two: segment 13 and 14 are new.
        let slid = LIVE
            .replace("MEDIA-SEQUENCE:10", "MEDIA-SEQUENCE:12")
            .replace("s10.ts", "s12.ts")
            .replace("s11.ts", "s13.ts")
            .replace("s12.ts\n", "s14.ts\n");
        assert!(c.update(media(&slid)));
        assert_eq!(c.take().map(|s| s.sequence), Some(13));
    }

    #[test]
    fn cursor_jumps_forward_when_it_fell_behind() {
        let mut c = Cursor::new(media(LIVE), 0);
        let far = LIVE.replace("MEDIA-SEQUENCE:10", "MEDIA-SEQUENCE:50");
        assert!(c.update(media(&far)));
        assert_eq!(c.take().map(|s| s.sequence), Some(50));
    }

    #[test]
    fn cursor_rejoins_live_edge_after_numbering_reset() {
        let mut c = Cursor::new(media(LIVE), 3);
        assert_eq!(c.next_seq, 13);
        let reset = LIVE.replace("MEDIA-SEQUENCE:10", "MEDIA-SEQUENCE:0");
        assert!(c.update(media(&reset)));
        assert_eq!(c.take().map(|s| s.sequence), Some(0));
    }

    fn source_with(chunks: Vec<io::Result<Vec<u8>>>, first: &[u8]) -> HlsSource {
        let (tx, rx) = sync_channel(8);
        for c in chunks {
            tx.send(c).expect("send");
        }
        drop(tx);
        HlsSource::new(rx, first.to_vec(), Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn source_concatenates_chunks_then_eof() {
        let mut s = source_with(vec![Ok(vec![4, 5, 6]), Ok(vec![]), Ok(vec![7])], &[1, 2, 3]);
        let mut all = Vec::new();
        let mut buf = [0u8; 2];
        loop {
            let n = s.read(&mut buf).expect("read");
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(s.read(&mut buf).expect("eof"), 0);
        assert!(!s.is_seekable());
        assert!(s.byte_len().is_none());
    }

    #[test]
    fn source_surfaces_worker_error_once_then_eof() {
        let mut s = source_with(vec![Err(io::Error::other("boom"))], &[1]);
        let mut buf = [0u8; 8];
        assert_eq!(s.read(&mut buf).expect("first"), 1);
        assert!(s.read(&mut buf).is_err());
        assert_eq!(s.read(&mut buf).expect("eof"), 0);
    }

    #[test]
    fn dropping_source_raises_stop_flag() {
        let stop = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = sync_channel::<io::Result<Vec<u8>>>(1);
        drop(HlsSource::new(rx, Vec::new(), Arc::clone(&stop)));
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn sleep_is_interrupted_by_stop() {
        let stop = AtomicBool::new(true);
        let t = Instant::now();
        assert!(!sleep_interruptible(&stop, Duration::from_secs(30)));
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(sleep_interruptible(
            &AtomicBool::new(false),
            Duration::from_millis(1)
        ));
    }

    #[test]
    fn unsupported_key_method_error_kind() {
        let client = reqwest::blocking::Client::new();
        let now = Instant::now();
        let mut w = Worker {
            client,
            media_url: "https://h/x.m3u8".to_owned(),
            cursor: Cursor::new(media(LIVE), 0),
            stop: Arc::new(AtomicBool::new(false)),
            keys: HashMap::new(),
            current_map: None,
            proc: SegmentProcessor::default(),
            last_reload: now,
            last_progress: now,
            empty_reloads: 0,
            reload_errors: 0,
        };
        let key = KeyInfo {
            method: KeyMethod::Unsupported("SAMPLE-AES".to_owned()),
            uri: Some("https://h/k".to_owned()),
            iv: None,
        };
        let err = w.decrypt(&[0u8; 16], &key, 0).expect_err("unsupported");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        assert!(err.to_string().contains("SAMPLE-AES"));
    }
}
