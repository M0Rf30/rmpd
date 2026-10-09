// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Native FLAC stream encoder (pure Rust, no external dependencies).
//!
//! Produces a *streamable* FLAC bit-stream: the `fLaC` marker and a
//! STREAMINFO block (unknown total sample count, no MD5) are sent as the
//! stream header, followed by independent fixed-size frames of
//! [`BLOCK_SIZE`] sample frames each.  Every frame is decodable on its own,
//! so a client that joins mid-stream only needs the header.
//!
//! Input is quantised to 16 bits.  Each channel is coded with the smallest of
//! a CONSTANT, VERBATIM or FIXED-predictor (order 0–4) subframe with
//! partitioned Rice residual coding; the `compression` level (0–8) selects how
//! many predictor orders / partition orders are tried (0 = verbatim only).

use super::{Encoder, f32_to_i16};
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;

/// Samples per channel in every frame (FLAC block-size code 12).
pub const BLOCK_SIZE: usize = 4096;
const BLOCK_SIZE_CODE: u8 = 12;
const BITS_PER_SAMPLE: u32 = 16;
/// Default compression level (mirrors the reference `flac` encoder).
pub const DEFAULT_COMPRESSION: u8 = 5;
/// Highest accepted compression level.
pub const MAX_COMPRESSION: u8 = 8;
/// Largest Rice parameter usable with the 4-bit coding method.
const MAX_RICE_PARAM: u32 = 14;

// ── CRCs ─────────────────────────────────────────────────────────────────────

/// CRC-8 (polynomial 0x07) over a frame header.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 (polynomial 0x8005, init 0) over a whole frame.
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// FLAC's extended UTF-8 coding of the frame number (up to 36 bits).
fn utf8_encode(v: u64, out: &mut Vec<u8>) {
    if v < 0x80 {
        out.push(v as u8);
        return;
    }
    let cont: u32 = if v < 1 << 11 {
        1
    } else if v < 1 << 16 {
        2
    } else if v < 1 << 21 {
        3
    } else if v < 1 << 26 {
        4
    } else if v < 1 << 31 {
        5
    } else {
        6
    };
    let total = cont + 1;
    let prefix = (0xFFu32 << (8 - total)) as u8;
    out.push(prefix | (v >> (6 * cont)) as u8);
    for i in 0..cont {
        let shift = 6 * (cont - 1 - i);
        out.push(0x80 | ((v >> shift) & 0x3F) as u8);
    }
}

// ── Bit writer ───────────────────────────────────────────────────────────────

struct BitWriter {
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(BLOCK_SIZE * 4),
            acc: 0,
            nbits: 0,
        }
    }

    /// Append the low `n` (≤ 32) bits of `value`, MSB first.
    fn write(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 32);
        if n == 0 {
            return;
        }
        self.acc = (self.acc << n) | (value & ((1u64 << n) - 1));
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    fn write_signed(&mut self, v: i32, n: u32) {
        self.write(u64::from(v as u32), n);
    }

    /// `q` zero bits followed by a one bit.
    fn write_unary(&mut self, mut q: u32) {
        while q >= 32 {
            self.write(0, 32);
            q -= 32;
        }
        self.write(0, q);
        self.write(1, 1);
    }

    fn align(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.write(0, pad);
        }
    }
}

// ── Subframe coding ──────────────────────────────────────────────────────────

/// A fully-evaluated FIXED subframe candidate.
struct FixedCandidate {
    order: usize,
    partition_order: u32,
    /// One Rice parameter per partition.
    params: Vec<u32>,
    /// Zig-zag folded residuals (`BLOCK_SIZE - order` values).
    residual: Vec<u32>,
    bits: u64,
}

/// Best Rice parameter (and its cost in bits) for a partition of folded values.
fn best_rice(vals: &[u32]) -> (u32, u64) {
    let n = vals.len() as u64;
    if n == 0 {
        return (0, 0);
    }
    let sum: u64 = vals.iter().map(|&v| u64::from(v)).sum();
    let mean = sum / n;
    let est = if mean == 0 {
        0
    } else {
        (63 - mean.leading_zeros()).min(MAX_RICE_PARAM)
    };
    let lo = est.saturating_sub(1);
    let hi = (est + 1).min(MAX_RICE_PARAM);
    let mut best = (lo, u64::MAX);
    for p in lo..=hi {
        let mut bits = n * (u64::from(p) + 1);
        for &v in vals {
            bits += u64::from(v >> p);
        }
        if bits < best.1 {
            best = (p, bits);
        }
    }
    best
}

/// Evaluate partition orders `0..=max_po` for `residual` (already folded) and
/// return the cheapest `(partition_order, params, bits)`.
fn best_partitioning(
    residual: &[u32],
    order: usize,
    max_po: u32,
    n: usize,
) -> (u32, Vec<u32>, u64) {
    let mut best: Option<(u32, Vec<u32>, u64)> = None;
    for po in 0..=max_po {
        let parts = 1usize << po;
        if n & (parts - 1) != 0 {
            break;
        }
        let part_len = n >> po;
        if part_len <= order {
            break;
        }
        let mut idx = 0usize;
        let mut params = Vec::with_capacity(parts);
        // 2 bits method + 4 bits partition order.
        let mut bits = 6u64;
        for p in 0..parts {
            let len = if p == 0 { part_len - order } else { part_len };
            let (param, pbits) = best_rice(&residual[idx..idx + len]);
            idx += len;
            params.push(param);
            bits += 4 + pbits;
        }
        if best.as_ref().is_none_or(|b| bits < b.2) {
            best = Some((po, params, bits));
        }
    }
    best.expect("partition order 0 is always valid")
}

fn write_subframe(bw: &mut BitWriter, samples: &[i32], level: u8) {
    let n = samples.len();
    debug_assert!((1..=BLOCK_SIZE).contains(&n));

    // CONSTANT (digital silence / DC).
    if samples.iter().all(|&s| s == samples[0]) {
        bw.write(0, 1); // padding
        bw.write(0, 6); // type: CONSTANT
        bw.write(0, 1); // no wasted bits
        bw.write_signed(samples[0], BITS_PER_SAMPLE);
        return;
    }

    let verbatim_bits = 8 + (n as u64) * u64::from(BITS_PER_SAMPLE);
    let mut best_fixed: Option<FixedCandidate> = None;

    if level > 0 {
        let max_order = (if level <= 2 { 2 } else { 4 }).min(n - 1);
        let max_po = match level {
            1..=2 => 3,
            3..=5 => 5,
            _ => 6,
        };
        let mut cur: Vec<i64> = samples.iter().map(|&s| i64::from(s)).collect();
        for order in 0..=max_order {
            if order > 0 {
                for i in (1..cur.len()).rev() {
                    cur[i] -= cur[i - 1];
                }
            }
            let residual: Vec<u32> = cur[order..]
                .iter()
                .map(|&r| ((r << 1) ^ (r >> 63)) as u32)
                .collect();
            let (partition_order, params, rbits) = best_partitioning(&residual, order, max_po, n);
            let bits = 8 + (order as u64) * u64::from(BITS_PER_SAMPLE) + rbits;
            if best_fixed.as_ref().is_none_or(|b| bits < b.bits) {
                best_fixed = Some(FixedCandidate {
                    order,
                    partition_order,
                    params,
                    residual,
                    bits,
                });
            }
        }
    }

    match best_fixed {
        Some(f) if f.bits < verbatim_bits => {
            bw.write(0, 1);
            bw.write(0b001000 + f.order as u64, 6);
            bw.write(0, 1);
            for &s in &samples[..f.order] {
                bw.write_signed(s, BITS_PER_SAMPLE);
            }
            bw.write(0, 2); // residual coding method: 4-bit Rice parameters
            bw.write(u64::from(f.partition_order), 4);
            let part_len = samples.len() >> f.partition_order;
            let mut idx = 0usize;
            for (p, &param) in f.params.iter().enumerate() {
                let len = if p == 0 { part_len - f.order } else { part_len };
                bw.write(u64::from(param), 4);
                for &u in &f.residual[idx..idx + len] {
                    bw.write_unary(u >> param);
                    bw.write(u64::from(u), param);
                }
                idx += len;
            }
        }
        _ => {
            bw.write(0, 1);
            bw.write(0b000001, 6); // VERBATIM
            bw.write(0, 1);
            for &s in samples {
                bw.write_signed(s, BITS_PER_SAMPLE);
            }
        }
    }
}

// ── Encoder ──────────────────────────────────────────────────────────────────

/// Streaming FLAC encoder (see module docs).
pub struct FlacEncoder {
    channels: usize,
    sample_rate: u32,
    level: u8,
    /// Interleaved 16-bit samples waiting for a full block.
    pending: Vec<i32>,
    frame_number: u64,
}

impl FlacEncoder {
    /// Create an encoder for `format` with a `compression` level (0–8,
    /// clamped).
    ///
    /// # Errors
    /// FLAC supports 1–8 channels and sample rates up to 1 048 575 Hz.
    pub fn new(format: AudioFormat, level: u8) -> Result<Self> {
        let channels = usize::from(format.channels);
        if !(1..=8).contains(&channels) {
            return Err(RmpdError::Player(format!(
                "flac encoder supports 1-8 channels, got {channels}"
            )));
        }
        if format.sample_rate == 0 || format.sample_rate > 0xF_FFFF {
            return Err(RmpdError::Player(format!(
                "flac encoder: unsupported sample rate {}",
                format.sample_rate
            )));
        }
        Ok(Self {
            channels,
            sample_rate: format.sample_rate,
            level: level.min(MAX_COMPRESSION),
            pending: Vec::with_capacity(BLOCK_SIZE * channels * 2),
            frame_number: 0,
        })
    }

    fn stream_header(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(42);
        h.extend_from_slice(b"fLaC");
        // Metadata block header: last-block flag set, type 0 (STREAMINFO), len 34.
        h.push(0x80);
        h.extend_from_slice(&[0, 0, 34]);
        h.extend_from_slice(&(BLOCK_SIZE as u16).to_be_bytes()); // min block size
        h.extend_from_slice(&(BLOCK_SIZE as u16).to_be_bytes()); // max block size
        h.extend_from_slice(&[0, 0, 0]); // min frame size (unknown)
        h.extend_from_slice(&[0, 0, 0]); // max frame size (unknown)
        // 20-bit sample rate | 3-bit (channels-1) | 5-bit (bps-1) | 36-bit total samples
        let packed: u64 = (u64::from(self.sample_rate) << 44)
            | (((self.channels as u64) - 1) << 41)
            | (u64::from(BITS_PER_SAMPLE - 1) << 36);
        h.extend_from_slice(&packed.to_be_bytes());
        h.extend_from_slice(&[0u8; 16]); // MD5 (unknown)
        h
    }

    /// Encode one frame of `block.len() / channels` sample frames (a full
    /// [`BLOCK_SIZE`] block, or the shorter final block from `finish`).
    fn encode_frame(&mut self, block: &[i32]) -> Vec<u8> {
        let ch = self.channels;
        let n = block.len() / ch;
        debug_assert!((1..=BLOCK_SIZE).contains(&n) && block.len() == n * ch);
        let full = n == BLOCK_SIZE;

        let mut head = vec![
            0xFF,
            0xF8, // sync + fixed block size strategy
            // block size code (7 = explicit 16-bit value follows); sample rate from STREAMINFO
            (if full { BLOCK_SIZE_CODE } else { 7 }) << 4,
            (((ch as u8) - 1) << 4) | (0b100 << 1), // independent channels, 16 bps
        ];
        utf8_encode(self.frame_number, &mut head);
        if !full {
            head.extend_from_slice(&((n - 1) as u16).to_be_bytes());
        }
        head.push(crc8(&head));

        let mut bw = BitWriter::new();
        let mut chan = vec![0i32; n];
        for c in 0..ch {
            for (i, s) in chan.iter_mut().enumerate() {
                *s = block[i * ch + c];
            }
            write_subframe(&mut bw, &chan, self.level);
        }
        bw.align();

        let mut frame = head;
        frame.extend_from_slice(&bw.buf);
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        self.frame_number += 1;
        frame
    }
}

impl Encoder for FlacEncoder {
    fn content_type(&self) -> &str {
        "audio/flac"
    }

    fn header(&self) -> Vec<u8> {
        self.stream_header()
    }

    fn encode(&mut self, samples: &[f32]) -> Vec<u8> {
        self.pending
            .extend(samples.iter().map(|&s| i32::from(f32_to_i16(s))));
        let block_len = BLOCK_SIZE * self.channels;
        let mut out = Vec::new();
        while self.pending.len() >= block_len {
            let block: Vec<i32> = self.pending.drain(..block_len).collect();
            out.extend(self.encode_frame(&block));
        }
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        let frames = self.pending.len() / self.channels;
        let mut pending = std::mem::take(&mut self.pending);
        pending.truncate(frames * self.channels);
        let out = if frames == 0 {
            Vec::new()
        } else {
            self.encode_frame(&pending)
        };
        pending.clear();
        self.pending = pending;
        out
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.frame_number = 0;
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(channels: u8, rate: u32) -> AudioFormat {
        AudioFormat {
            sample_rate: rate,
            channels,
            bits_per_sample: 16,
        }
    }

    // ── minimal reference decoder (test only) ───────────────────────────────

    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl BitReader<'_> {
        fn read(&mut self, n: u32) -> u64 {
            let mut v = 0u64;
            for _ in 0..n {
                let byte = self.data[self.pos / 8];
                let bit = (byte >> (7 - (self.pos % 8))) & 1;
                v = (v << 1) | u64::from(bit);
                self.pos += 1;
            }
            v
        }
        fn read_signed(&mut self, n: u32) -> i32 {
            let v = self.read(n);
            let shift = 64 - n;
            (((v << shift) as i64) >> shift) as i32
        }
        fn read_unary(&mut self) -> u32 {
            let mut q = 0;
            while self.read(1) == 0 {
                q += 1;
            }
            q
        }
    }

    /// Decode one frame; returns interleaved samples and the frame length.
    fn decode_frame(data: &[u8], channels: usize) -> (Vec<i32>, usize) {
        assert_eq!(&data[..2], &[0xFF, 0xF8], "sync code");
        let code = data[2] >> 4;
        assert!(
            code == BLOCK_SIZE_CODE || code == 7,
            "block size code {code}"
        );
        assert_eq!(data[3] >> 4, (channels - 1) as u8);
        // UTF-8 frame number length.
        let first = data[4];
        let utf_len = if first < 0x80 {
            1
        } else {
            first.leading_ones() as usize
        };
        let (bs, hlen) = if code == 7 {
            let at = 4 + utf_len;
            (
                usize::from(u16::from_be_bytes([data[at], data[at + 1]])) + 1,
                at + 3,
            )
        } else {
            (BLOCK_SIZE, 4 + utf_len + 1)
        };
        assert_eq!(crc8(&data[..hlen - 1]), data[hlen - 1], "header crc8");

        let mut br = BitReader {
            data,
            pos: hlen * 8,
        };
        let mut out = vec![0i32; bs * channels];
        for c in 0..channels {
            assert_eq!(br.read(1), 0);
            let t = br.read(6);
            assert_eq!(br.read(1), 0, "no wasted bits");
            let mut s = vec![0i32; bs];
            match t {
                0 => {
                    let v = br.read_signed(16);
                    s.iter_mut().for_each(|x| *x = v);
                }
                1 => {
                    for x in s.iter_mut() {
                        *x = br.read_signed(16);
                    }
                }
                8..=12 => {
                    let order = (t - 8) as usize;
                    for x in s.iter_mut().take(order) {
                        *x = br.read_signed(16);
                    }
                    assert_eq!(br.read(2), 0, "rice method 0");
                    let po = br.read(4) as u32;
                    let part_len = bs >> po;
                    let mut i = order;
                    for p in 0..(1usize << po) {
                        let len = if p == 0 { part_len - order } else { part_len };
                        let param = br.read(4) as u32;
                        assert!(param <= MAX_RICE_PARAM);
                        for _ in 0..len {
                            let q = br.read_unary();
                            let u = (q << param) | br.read(param) as u32;
                            let r = ((u >> 1) as i32) ^ -((u & 1) as i32);
                            let pred = match order {
                                0 => 0,
                                1 => s[i - 1],
                                2 => 2 * s[i - 1] - s[i - 2],
                                3 => 3 * s[i - 1] - 3 * s[i - 2] + s[i - 3],
                                _ => 4 * s[i - 1] - 6 * s[i - 2] + 4 * s[i - 3] - s[i - 4],
                            };
                            s[i] = pred + r;
                            i += 1;
                        }
                    }
                    assert_eq!(i, bs);
                }
                other => panic!("unexpected subframe type {other}"),
            }
            for (i, v) in s.iter().enumerate() {
                out[i * channels + c] = *v;
            }
        }
        // Byte alignment padding must be zero; CRC-16 follows.
        let body_end = br.pos.div_ceil(8);
        let crc = u16::from_be_bytes([data[body_end], data[body_end + 1]]);
        assert_eq!(crc16(&data[..body_end]), crc, "frame crc16");
        (out, body_end + 2)
    }

    fn pseudo_noise(n: usize, amp: i32, seed: &mut u32) -> Vec<i32> {
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((*seed >> 16) as i32 % (2 * amp + 1)) - amp
            })
            .collect()
    }

    fn to_f32(v: &[i32]) -> Vec<f32> {
        v.iter().map(|&s| s as f32 / 32767.0).collect()
    }

    // ── header ──────────────────────────────────────────────────────────────

    #[test]
    fn stream_header_is_valid_streaminfo() {
        let enc = FlacEncoder::new(fmt(2, 44100), 5).unwrap();
        let h = enc.header();
        assert_eq!(h.len(), 42);
        assert_eq!(&h[..4], b"fLaC");
        assert_eq!(h[4], 0x80, "last-block flag + STREAMINFO type");
        assert_eq!(&h[5..8], &[0, 0, 34], "STREAMINFO length");
        assert_eq!(u16::from_be_bytes([h[8], h[9]]), BLOCK_SIZE as u16);
        assert_eq!(u16::from_be_bytes([h[10], h[11]]), BLOCK_SIZE as u16);
        let packed = u64::from_be_bytes(h[18..26].try_into().unwrap());
        assert_eq!(packed >> 44, 44100, "sample rate");
        assert_eq!((packed >> 41) & 0x7, 1, "channels - 1");
        assert_eq!((packed >> 36) & 0x1F, 15, "bits per sample - 1");
        assert_eq!(packed & 0xF_FFFF_FFFF, 0, "unknown total samples");
        assert_eq!(&h[26..42], &[0u8; 16], "MD5 unset");
    }

    #[test]
    fn rejects_unsupported_formats() {
        assert!(FlacEncoder::new(fmt(0, 44100), 5).is_err());
        assert!(FlacEncoder::new(fmt(9, 44100), 5).is_err());
        assert!(FlacEncoder::new(fmt(2, 0), 5).is_err());
        assert!(FlacEncoder::new(fmt(2, 2_000_000), 5).is_err());
    }

    #[test]
    fn content_type_is_flac() {
        let enc = FlacEncoder::new(fmt(2, 48000), 5).unwrap();
        assert_eq!(enc.content_type(), "audio/flac");
    }

    // ── framing ─────────────────────────────────────────────────────────────

    #[test]
    fn partial_blocks_are_buffered() {
        let mut enc = FlacEncoder::new(fmt(2, 44100), 5).unwrap();
        assert!(enc.encode(&vec![0.1; BLOCK_SIZE * 2 - 2]).is_empty());
        assert!(!enc.encode(&[0.1, 0.1]).is_empty());
    }

    #[test]
    fn utf8_frame_numbers() {
        let mut v = Vec::new();
        utf8_encode(0x7F, &mut v);
        assert_eq!(v, [0x7F]);
        v.clear();
        utf8_encode(0x80, &mut v);
        assert_eq!(v, [0xC2, 0x80]);
        v.clear();
        utf8_encode(0x800, &mut v);
        assert_eq!(v, [0xE0, 0xA0, 0x80]);
        v.clear();
        utf8_encode(0x1_0000, &mut v);
        assert_eq!(v, [0xF0, 0x90, 0x80, 0x80]);
    }

    #[test]
    fn crc_known_values() {
        // CRC-16/UMTS ("BUYPASS") check value for "123456789" is 0xFEE8.
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        // CRC-8/SMBUS check value is 0xF4.
        assert_eq!(crc8(b"123456789"), 0xF4);
    }

    // ── round trips ─────────────────────────────────────────────────────────

    fn roundtrip(channels: u8, level: u8, samples: &[i32]) -> usize {
        let mut enc = FlacEncoder::new(fmt(channels, 44100), level).unwrap();
        let bytes = enc.encode(&to_f32(samples));
        let (decoded, len) = decode_frame(&bytes, usize::from(channels));
        assert_eq!(len, bytes.len(), "exactly one frame expected");
        assert_eq!(decoded, samples);
        bytes.len()
    }

    #[test]
    fn roundtrip_noise_all_levels() {
        let mut seed = 1;
        let samples = pseudo_noise(BLOCK_SIZE * 2, 20000, &mut seed);
        for level in 0..=MAX_COMPRESSION {
            roundtrip(2, level, &samples);
        }
    }

    #[test]
    fn roundtrip_full_scale_extremes() {
        let samples: Vec<i32> = (0..BLOCK_SIZE)
            .map(|i| if i % 2 == 0 { 32767 } else { -32767 })
            .collect();
        for level in [0, 3, 8] {
            roundtrip(1, level, &samples);
        }
    }

    #[test]
    fn roundtrip_constant_block_is_tiny() {
        let samples = vec![0i32; BLOCK_SIZE * 2];
        let len = roundtrip(2, 5, &samples);
        assert!(len < 32, "silence frame should be tiny, got {len}");
    }

    #[test]
    fn smooth_signal_compresses_below_verbatim() {
        let samples: Vec<i32> = (0..BLOCK_SIZE)
            .map(|i| ((i as f64 * 0.05).sin() * 12000.0).round() as i32)
            .collect();
        let verbatim = roundtrip(1, 0, &samples);
        let packed = roundtrip(1, 5, &samples);
        assert!(packed < verbatim / 2, "{packed} vs {verbatim}");
    }

    #[test]
    fn frame_numbers_increase() {
        let mut enc = FlacEncoder::new(fmt(1, 44100), 5).unwrap();
        let mut seed = 7;
        let samples = to_f32(&pseudo_noise(BLOCK_SIZE * 3, 1000, &mut seed));
        let bytes = enc.encode(&samples);
        let mut off = 0;
        for expected in 0u8..3 {
            assert_eq!(bytes[off + 4], expected, "frame number");
            let (_, len) = decode_frame(&bytes[off..], 1);
            off += len;
        }
        assert_eq!(off, bytes.len());
    }

    #[test]
    fn finish_flushes_short_final_block() {
        let mut seed = 11;
        for channels in [1usize, 2] {
            for frames in [1usize, 2, 3, 5, 100, 4095, 4097, 5000, 8193] {
                for level in [0u8, 5, 8] {
                    let samples = pseudo_noise(frames * channels, 3000, &mut seed);
                    let mut enc = FlacEncoder::new(fmt(channels as u8, 44100), level).unwrap();
                    let mut bytes = enc.encode(&to_f32(&samples));
                    bytes.extend(enc.finish());
                    assert!(enc.finish().is_empty(), "finish is idempotent");
                    let mut decoded = Vec::new();
                    let mut off = 0;
                    while off < bytes.len() {
                        let (s, len) = decode_frame(&bytes[off..], channels);
                        decoded.extend(s);
                        off += len;
                    }
                    assert_eq!(decoded, samples, "{channels}ch {frames} frames L{level}");
                }
            }
        }
    }

    #[test]
    fn finish_without_pending_is_empty_and_reset_restarts() {
        let mut enc = FlacEncoder::new(fmt(1, 44100), 5).unwrap();
        assert!(enc.finish().is_empty());
        let mut seed = 3;
        let samples = to_f32(&pseudo_noise(BLOCK_SIZE + 10, 1000, &mut seed));
        let first = enc.encode(&samples);
        assert_eq!(first[4], 0);
        enc.reset();
        assert!(enc.finish().is_empty(), "reset drops pending samples");
        let again = enc.encode(&samples);
        assert_eq!(again, first, "frame numbering restarts after reset");
    }
}
