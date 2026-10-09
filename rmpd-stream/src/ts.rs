// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal MPEG transport stream (ISO/IEC 13818-1) audio demuxer for HLS.
//!
//! [`TsDemuxer`] follows PAT → PMT, picks the first audio elementary stream
//! (`stream_type` `0x0F` AAC/ADTS, `0x03`/`0x04` MPEG-1/2 audio) and
//! reassembles its PES payloads into a raw elementary stream that Symphonia's
//! ADTS / MP3 readers can parse. Everything else (video, data, timestamps) is
//! ignored. No external dependencies, no I/O.

/// Size of a transport stream packet.
pub const PACKET_SIZE: usize = 188;

const SYNC_BYTE: u8 = 0x47;
const PAT_PID: u16 = 0;

/// Kind of audio elementary stream extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    /// AAC with ADTS framing (`stream_type` 0x0F).
    AdtsAac,
    /// MPEG-1/2 audio layer I/II/III (`stream_type` 0x03 / 0x04).
    Mpeg,
}

/// Incremental transport stream demultiplexer for one audio stream.
#[derive(Debug, Default)]
pub struct TsDemuxer {
    /// Bytes carried over that did not yet form a whole packet.
    carry: Vec<u8>,
    pmt_pid: Option<u16>,
    audio_pid: Option<u16>,
    kind: Option<AudioKind>,
    /// The PES packet being assembled (starts at its `00 00 01` prefix).
    pes: Vec<u8>,
    /// Finished elementary-stream bytes not yet handed out.
    out: Vec<u8>,
}

impl TsDemuxer {
    /// A demuxer with no knowledge of the stream yet.
    #[cfg(test)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The audio kind found in the PMT, once seen.
    #[must_use]
    pub fn kind(&self) -> Option<AudioKind> {
        self.kind
    }

    /// Forget partial packet/PES state (stream discontinuity). PAT/PMT
    /// knowledge is kept: it is re-announced in every segment anyway.
    pub fn reset(&mut self) {
        self.carry.clear();
        self.pes.clear();
    }

    /// Feed more transport stream bytes; returns the elementary-stream bytes
    /// completed so far.
    pub fn push(&mut self, data: &[u8]) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(data);
        let mut pos = 0;
        while buf.len() - pos >= PACKET_SIZE {
            if buf[pos] != SYNC_BYTE || !self.in_sync(&buf, pos) {
                // Lost sync: hunt for the next plausible packet boundary.
                match find_sync(&buf[pos + 1..]) {
                    Some(skip) => pos += 1 + skip,
                    None => {
                        pos = buf.len().saturating_sub(PACKET_SIZE - 1);
                        break;
                    }
                }
                continue;
            }
            self.packet(&buf[pos..pos + PACKET_SIZE]);
            pos += PACKET_SIZE;
        }
        self.carry = buf.split_off(pos);
        std::mem::take(&mut self.out)
    }

    /// Flush the PES packet in progress (end of segment / stream).
    pub fn finish(&mut self) -> Vec<u8> {
        self.finish_pes();
        self.carry.clear();
        std::mem::take(&mut self.out)
    }

    /// A sync byte at `pos` counts only when the next packet (if buffered)
    /// also starts with one.
    fn in_sync(&self, buf: &[u8], pos: usize) -> bool {
        buf.get(pos + PACKET_SIZE).is_none_or(|b| *b == SYNC_BYTE)
    }

    fn packet(&mut self, pkt: &[u8]) {
        let pusi = pkt[1] & 0x40 != 0;
        let tei = pkt[1] & 0x80 != 0;
        let pid = (u16::from(pkt[1] & 0x1F) << 8) | u16::from(pkt[2]);
        let scrambled = pkt[3] & 0xC0 != 0;
        let afc = (pkt[3] >> 4) & 0x3;
        if tei || scrambled || afc & 0x1 == 0 {
            return;
        }
        let mut off = 4;
        if afc & 0x2 != 0 {
            off += 1 + usize::from(pkt[4]);
        }
        if off >= PACKET_SIZE {
            return;
        }
        let payload = &pkt[off..];
        if pid == PAT_PID {
            self.pat(payload, pusi);
        } else if Some(pid) == self.pmt_pid {
            self.pmt(payload, pusi);
        } else if Some(pid) == self.audio_pid {
            if pusi {
                self.finish_pes();
            }
            // Ignore payload until the first PES start has been seen.
            if pusi || !self.pes.is_empty() {
                self.pes.extend_from_slice(payload);
            }
        }
    }

    /// The PSI section in `payload` (single-packet sections only).
    fn section(payload: &[u8], pusi: bool) -> Option<&[u8]> {
        if !pusi {
            return None;
        }
        let pointer = usize::from(*payload.first()?);
        let sec = payload.get(1 + pointer..)?;
        if sec.len() < 3 {
            return None;
        }
        let len = (usize::from(sec[1] & 0x0F) << 8) | usize::from(sec[2]);
        sec.get(..3 + len)
    }

    fn pat(&mut self, payload: &[u8], pusi: bool) {
        let Some(sec) = Self::section(payload, pusi) else {
            return;
        };
        if sec[0] != 0x00 || sec.len() < 12 {
            return;
        }
        // Program entries follow the 8-byte header and precede the CRC.
        for entry in sec[8..sec.len() - 4].as_chunks::<4>().0 {
            let program = u16::from_be_bytes([entry[0], entry[1]]);
            let pid = (u16::from(entry[2] & 0x1F) << 8) | u16::from(entry[3]);
            if program != 0 {
                self.pmt_pid = Some(pid);
                return;
            }
        }
    }

    fn pmt(&mut self, payload: &[u8], pusi: bool) {
        if self.audio_pid.is_some() {
            return;
        }
        let Some(sec) = Self::section(payload, pusi) else {
            return;
        };
        if sec[0] != 0x02 || sec.len() < 16 {
            return;
        }
        let program_info = (usize::from(sec[10] & 0x0F) << 8) | usize::from(sec[11]);
        let end = sec.len() - 4;
        let mut i = 12 + program_info;
        while i + 5 <= end {
            let stream_type = sec[i];
            let pid = (u16::from(sec[i + 1] & 0x1F) << 8) | u16::from(sec[i + 2]);
            let es_info = (usize::from(sec[i + 3] & 0x0F) << 8) | usize::from(sec[i + 4]);
            let kind = match stream_type {
                0x0F => Some(AudioKind::AdtsAac),
                0x03 | 0x04 => Some(AudioKind::Mpeg),
                _ => None,
            };
            if let Some(kind) = kind {
                self.audio_pid = Some(pid);
                self.kind = Some(kind);
                return;
            }
            i += 5 + es_info;
        }
    }

    /// Strip the PES header from the assembled packet and queue its payload.
    fn finish_pes(&mut self) {
        let pes = std::mem::take(&mut self.pes);
        if pes.len() < 9 || pes[..3] != [0x00, 0x00, 0x01] {
            return;
        }
        let start = 9 + usize::from(pes[8]);
        let declared = (usize::from(pes[4]) << 8) | usize::from(pes[5]);
        // A non-zero PES_packet_length bounds the packet; trailing stuffing
        // beyond it must not leak into the elementary stream.
        let end = if declared > 0 {
            (6 + declared).min(pes.len())
        } else {
            pes.len()
        };
        if start < end {
            self.out.extend_from_slice(&pes[start..end]);
        }
    }
}

/// Offset of the first byte in `data` that starts a run of packets (a sync
/// byte repeated at 188-byte spacing, as far as the data reaches).
fn find_sync(data: &[u8]) -> Option<usize> {
    (0..data.len()).find(|&i| {
        data[i] == SYNC_BYTE && data.get(i + PACKET_SIZE).is_none_or(|b| *b == SYNC_BYTE)
    })
}

/// Whether `data` looks like the start of a transport stream (sync bytes at
/// 188-byte spacing).
#[must_use]
pub fn looks_like_ts(data: &[u8]) -> bool {
    data.first() == Some(&SYNC_BYTE)
        && (data.len() < PACKET_SIZE + 1 || data[PACKET_SIZE] == SYNC_BYTE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap `payload` (≤ 184 bytes) into one 188-byte packet, padding with an
    /// adaptation field when shorter.
    fn packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= 184);
        let mut p = vec![SYNC_BYTE, (pid >> 8) as u8 & 0x1F, (pid & 0xFF) as u8];
        if pusi {
            p[1] |= 0x40;
        }
        if payload.len() == 184 {
            p.push(0x10);
        } else {
            p.push(0x30);
            let af_len = 183 - payload.len();
            p.push(af_len as u8);
            if af_len > 0 {
                p.push(0x00);
                p.extend(std::iter::repeat_n(0xFF, af_len - 1));
            }
        }
        p.extend_from_slice(payload);
        assert_eq!(p.len(), PACKET_SIZE);
        p
    }

    fn pat(pmt_pid: u16) -> Vec<u8> {
        let mut s = vec![0x00, 0xB0, 13, 0x00, 0x01, 0xC1, 0x00, 0x00];
        s.extend_from_slice(&[0x00, 0x01, 0xE0 | (pmt_pid >> 8) as u8, pmt_pid as u8]);
        s.extend_from_slice(&[0, 0, 0, 0]); // CRC (not verified)
        let mut payload = vec![0x00];
        payload.extend(s);
        packet(PAT_PID, true, &payload)
    }

    /// PMT with the given `(stream_type, pid)` entries.
    fn pmt(pmt_pid: u16, streams: &[(u8, u16)]) -> Vec<u8> {
        let mut body = vec![0x00, 0x01, 0xC1, 0x00, 0x00, 0xE1, 0x00, 0xF0, 0x00];
        for (ty, pid) in streams {
            body.extend_from_slice(&[*ty, 0xE0 | (*pid >> 8) as u8, *pid as u8, 0xF0, 0x00]);
        }
        body.extend_from_slice(&[0, 0, 0, 0]); // CRC
        let mut s = vec![0x02, 0xB0, body.len() as u8];
        s.extend(body);
        let mut payload = vec![0x00];
        payload.extend(s);
        packet(pmt_pid, true, &payload)
    }

    /// A PES packet (with a PTS-sized optional header) split over TS packets.
    fn pes_packets(pid: u16, es: &[u8]) -> Vec<u8> {
        let header_data = [0x21, 0x00, 0x01, 0x00, 0x01]; // fake PTS
        let mut pes = vec![0x00, 0x00, 0x01, 0xC0];
        let len = 3 + header_data.len() + es.len();
        pes.extend_from_slice(&(len as u16).to_be_bytes());
        pes.extend_from_slice(&[0x80, 0x80, header_data.len() as u8]);
        pes.extend_from_slice(&header_data);
        pes.extend_from_slice(es);
        let mut out = Vec::new();
        for (i, chunk) in pes.chunks(184).enumerate() {
            out.extend(packet(pid, i == 0, chunk));
        }
        out
    }

    fn stream(es_parts: &[&[u8]], audio_type: u8) -> Vec<u8> {
        let mut ts = pat(0x100);
        ts.extend(pmt(0x100, &[(0x1B, 0x101), (audio_type, 0x102)]));
        for part in es_parts {
            // Video packets for another PID must be ignored.
            ts.extend(pes_packets(0x101, &[0xAA; 50]));
            ts.extend(pes_packets(0x102, part));
        }
        ts
    }

    #[test]
    fn extracts_adts_audio_pid_from_synthetic_stream() {
        let frame_a: Vec<u8> = std::iter::once(0xFF)
            .chain(std::iter::once(0xF1))
            .chain((0..400).map(|i| (i % 251) as u8))
            .collect();
        let frame_b = vec![0xFF, 0xF1, 1, 2, 3];
        let ts = stream(&[&frame_a, &frame_b], 0x0F);
        let mut d = TsDemuxer::new();
        let mut out = d.push(&ts);
        out.extend(d.finish());
        assert_eq!(d.kind(), Some(AudioKind::AdtsAac));
        let mut want = frame_a;
        want.extend(&frame_b);
        assert_eq!(out, want);
    }

    #[test]
    fn mpeg_audio_stream_type_is_detected() {
        let es = [0xFF, 0xFB, 0x90, 0x00, 1, 2, 3];
        let ts = stream(&[&es], 0x03);
        let mut d = TsDemuxer::new();
        let mut out = d.push(&ts);
        out.extend(d.finish());
        assert_eq!(d.kind(), Some(AudioKind::Mpeg));
        assert_eq!(out, es);
    }

    #[test]
    fn incremental_feeding_in_odd_chunks_matches_one_shot() {
        let es: Vec<u8> = (0..1000).map(|i| (i * 7 % 256) as u8).collect();
        let ts = stream(&[&es], 0x0F);
        let mut d = TsDemuxer::new();
        let mut out = Vec::new();
        for chunk in ts.chunks(101) {
            out.extend(d.push(chunk));
        }
        out.extend(d.finish());
        assert_eq!(out, es);
    }

    #[test]
    fn resynchronizes_after_garbage() {
        let es = [0xFF, 0xF1, 9, 9, 9, 9];
        let mut ts = vec![0x00, 0x13, 0x37, 0x47, 0x00];
        ts.extend(stream(&[&es], 0x0F));
        let mut d = TsDemuxer::new();
        let mut out = d.push(&ts);
        out.extend(d.finish());
        assert_eq!(out, es);
    }

    #[test]
    fn stream_without_supported_audio_yields_nothing() {
        let mut ts = pat(0x100);
        ts.extend(pmt(0x100, &[(0x1B, 0x101)]));
        ts.extend(pes_packets(0x101, &[1, 2, 3]));
        let mut d = TsDemuxer::new();
        let mut out = d.push(&ts);
        out.extend(d.finish());
        assert!(out.is_empty());
        assert_eq!(d.kind(), None);
    }

    #[test]
    fn pes_length_bounds_trailing_stuffing() {
        // PES declares 5 payload bytes; extra bytes after it are stuffing.
        let mut pes = vec![0x00, 0x00, 0x01, 0xC0];
        pes.extend_from_slice(&(3u16 + 5).to_be_bytes());
        pes.extend_from_slice(&[0x80, 0x00, 0x00, 1, 2, 3, 4, 5, 0xFF, 0xFF]);
        let mut ts = pat(0x100);
        ts.extend(pmt(0x100, &[(0x0F, 0x102)]));
        ts.extend(packet(0x102, true, &pes));
        let mut d = TsDemuxer::new();
        let mut out = d.push(&ts);
        out.extend(d.finish());
        assert_eq!(out, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn ts_sniffing() {
        let ts = stream(&[&[1, 2, 3]], 0x0F);
        assert!(looks_like_ts(&ts));
        assert!(!looks_like_ts(&[0xFF, 0xF1, 0, 0]));
        assert!(!looks_like_ts(&[]));
        let mut bad = ts;
        bad[PACKET_SIZE] = 0;
        assert!(!looks_like_ts(&bad));
    }
}
