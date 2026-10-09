// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ogg Opus stream encoder (pure Rust, via `symphonia-codec-opus`).
//!
//! Produces a standard RFC 7845 Ogg Opus stream: the `OpusHead` and `OpusTags`
//! pages are the stream [header](Encoder::header), followed by audio pages.
//! The Opus codec only runs at 48 kHz, mono or stereo, so input at any other
//! rate is resampled to 48 kHz with the shared [`StreamResampler`] and
//! multichannel input (> 2 channels) is folded down to stereo first.
//!
//! [`Encoder::finish`] closes the stream with an end-of-stream page (the
//! granule position trims the decoder output to exactly the samples written);
//! [`Encoder::reset`] starts a fresh logical stream with a new Ogg serial
//! number and a new header.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use rmpd_core::config::ResamplerQuality;
use rmpd_core::error::{Result, RmpdError};
use rmpd_core::song::AudioFormat;
use symphonia_codec_opus::encoder::ogg::OggOpusWriter;
use symphonia_codec_opus::encoder::{BitrateMode, EncoderConfig, MAX_BITRATE, MIN_BITRATE};

use super::Encoder;
use crate::resampler::StreamResampler;

/// The only sample rate Opus encodes at.
pub const OPUS_RATE: u32 = 48_000;
/// Default target bitrate in kbit/s.
pub const DEFAULT_BITRATE_KBPS: u32 = 128;
/// Default encoder complexity (0–10). 9 is the library default: nearly the
/// quality of 10 at noticeably lower CPU use, which matters for real-time
/// streaming on small boards.
pub const DEFAULT_COMPLEXITY: u8 = 9;
/// Highest accepted complexity.
pub const MAX_COMPLEXITY: u8 = 10;

/// User-facing Opus settings of an `[[output]]` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusSettings {
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// Encoder complexity, 0 (fastest) – 10 (best).
    pub complexity: u8,
    /// Rate-control mode.
    pub mode: BitrateMode,
}

impl Default for OpusSettings {
    fn default() -> Self {
        Self {
            bitrate: DEFAULT_BITRATE_KBPS * 1000,
            complexity: DEFAULT_COMPLEXITY,
            mode: BitrateMode::Vbr,
        }
    }
}

impl OpusSettings {
    /// Settings with `kbps` clamped into the range the codec accepts.
    #[must_use]
    pub fn with_bitrate_kbps(mut self, kbps: f64) -> Self {
        let bps = (kbps * 1000.0).round();
        self.bitrate = bps.clamp(f64::from(MIN_BITRATE), f64::from(MAX_BITRATE)) as u32;
        self
    }

    /// Settings with `complexity` clamped to 0–10.
    #[must_use]
    pub fn with_complexity(mut self, complexity: f64) -> Self {
        self.complexity = complexity.clamp(0.0, f64::from(MAX_COMPLEXITY)).round() as u8;
        self
    }
}

/// Parse a `vbr` setting: `vbr`, `cvbr` (constrained VBR) or `cbr`; booleans
/// (`true` = `vbr`, `false` = `cbr`) are accepted too.
///
/// # Errors
/// Returns an error for any other value.
pub fn parse_mode(s: &str) -> Result<BitrateMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "vbr" | "true" | "on" | "yes" => Ok(BitrateMode::Vbr),
        "cvbr" | "constrained" | "constrained-vbr" | "constrained_vbr" => {
            Ok(BitrateMode::ConstrainedVbr)
        }
        "cbr" | "false" | "off" | "no" => Ok(BitrateMode::Cbr),
        other => Err(RmpdError::Player(format!(
            "opus encoder: invalid vbr mode '{other}' (expected vbr, cvbr or cbr)"
        ))),
    }
}

/// Unique-ish Ogg serial numbers: a clock-derived seed plus a per-stream counter.
fn next_serial() -> u32 {
    static SEED: LazyLock<u32> = LazyLock::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x6f70_7573, |d| d.subsec_nanos() ^ (d.as_secs() as u32))
    });
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    SEED.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed))
        .max(1)
}

/// A `Write` sink the Ogg writer fills and the encoder drains after each call.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl SharedSink {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock())
    }
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Per-input-channel `(left, right)` weights folding `n > 2` channels down to
/// stereo, normalised so a full-scale signal cannot clip.
fn downmix_weights(n: usize) -> Vec<(f32, f32)> {
    const C: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut w: Vec<(f32, f32)> = (0..n)
        .map(|i| match (n, i) {
            (_, 0) => (1.0, 0.0),
            (_, 1) => (0.0, 1.0),
            // 3.0: FL FR FC
            (3, 2) => (C, C),
            // quad: FL FR BL BR
            (4, 2) => (C, 0.0),
            (4, 3) => (0.0, C),
            // 5.0 / 5.1 / 7.1: FL FR FC LFE BL BR (SL SR ...)
            (_, 2) => (C, C),
            (_, 3) => (0.0, 0.0), // LFE
            (_, i) if i % 2 == 0 => (C, 0.0),
            _ => (0.0, C),
        })
        .collect();
    let sum_l: f32 = w.iter().map(|p| p.0).sum();
    let sum_r: f32 = w.iter().map(|p| p.1).sum();
    let norm = sum_l.max(sum_r).max(1.0);
    for p in &mut w {
        p.0 /= norm;
        p.1 /= norm;
    }
    w
}

/// Streaming Ogg Opus encoder (see module docs).
pub struct OpusEncoder {
    src_rate: u32,
    in_channels: usize,
    out_channels: u8,
    settings: OpusSettings,
    /// Folds `in_channels > 2` to stereo; `None` for mono/stereo input.
    downmix: Option<Vec<(f32, f32)>>,
    /// `None` when the input is already at 48 kHz.
    resampler: Option<StreamResampler>,
    writer: Option<OggOpusWriter<SharedSink>>,
    sink: SharedSink,
    header: Vec<u8>,
}

impl OpusEncoder {
    /// Create an encoder for input in `format` using `settings`.
    ///
    /// # Errors
    /// Fails for zero channels / zero sample rate, an invalid configuration, or
    /// when no resampler can be built for the input rate.
    pub fn new(format: AudioFormat, settings: OpusSettings) -> Result<Self> {
        let in_channels = usize::from(format.channels);
        if in_channels == 0 {
            return Err(RmpdError::Player(
                "opus encoder: input has no channels".to_owned(),
            ));
        }
        if format.sample_rate == 0 {
            return Err(RmpdError::Player(
                "opus encoder: unsupported sample rate 0".to_owned(),
            ));
        }
        let out_channels: u8 = if in_channels == 1 { 1 } else { 2 };
        let mut enc = Self {
            src_rate: format.sample_rate,
            in_channels,
            out_channels,
            settings,
            downmix: (in_channels > 2).then(|| downmix_weights(in_channels)),
            resampler: None,
            writer: None,
            sink: SharedSink::default(),
            header: Vec::new(),
        };
        enc.start_stream()?;
        Ok(enc)
    }

    fn config(&self) -> EncoderConfig {
        let mut cfg = EncoderConfig::new(self.out_channels, self.settings.bitrate);
        cfg.mode = self.settings.mode;
        cfg.complexity = self.settings.complexity;
        cfg
    }

    /// (Re)start a logical Ogg stream: fresh resampler state, new serial, new header.
    fn start_stream(&mut self) -> Result<()> {
        self.resampler = if self.src_rate == OPUS_RATE {
            None
        } else {
            Some(
                StreamResampler::new(
                    self.src_rate,
                    OPUS_RATE,
                    usize::from(self.out_channels),
                    ResamplerQuality::default(),
                )
                .ok_or_else(|| {
                    RmpdError::Player(format!(
                        "opus encoder: cannot resample {} Hz to {OPUS_RATE} Hz",
                        self.src_rate
                    ))
                })?,
            )
        };
        self.sink = SharedSink::default();
        let writer = OggOpusWriter::new(
            self.sink.clone(),
            next_serial(),
            self.config(),
            self.src_rate,
            &[("ENCODER", concat!("rmpd ", env!("CARGO_PKG_VERSION")))],
        )
        .map_err(|e| RmpdError::Player(format!("opus encoder: {e}")))?;
        self.writer = Some(writer);
        // The writer emits the OpusHead + OpusTags pages immediately.
        self.header = self.sink.take();
        Ok(())
    }

    /// Fold/convert `samples` to the 1- or 2-channel 48 kHz layout the codec takes.
    fn prepare(&mut self, samples: &[f32]) -> Vec<f32> {
        let ch = self.in_channels;
        let samples = &samples[..samples.len() / ch * ch];
        let pcm: Vec<f32> = match &self.downmix {
            Some(w) => {
                let mut out = Vec::with_capacity(samples.len() / ch * 2);
                for frame in samples.chunks_exact(ch) {
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for (s, (wl, wr)) in frame.iter().zip(w) {
                        l += s * wl;
                        r += s * wr;
                    }
                    out.push(l);
                    out.push(r);
                }
                out
            }
            None => samples.to_vec(),
        };
        match self.resampler.as_mut() {
            Some(rs) => rs.process(&pcm),
            None => pcm,
        }
    }
}

impl Encoder for OpusEncoder {
    fn content_type(&self) -> &str {
        "audio/ogg"
    }

    fn header(&self) -> Vec<u8> {
        self.header.clone()
    }

    fn output_format(&self, input: AudioFormat) -> AudioFormat {
        AudioFormat {
            sample_rate: OPUS_RATE,
            channels: self.out_channels,
            ..input
        }
    }

    fn encode(&mut self, samples: &[f32]) -> Vec<u8> {
        if self.writer.is_none() {
            // Encoding after `finish` without `reset`: begin a new stream anyway.
            if let Err(e) = self.start_stream() {
                tracing::error!("opus encoder restart failed: {e}");
                return Vec::new();
            }
        }
        let pcm = self.prepare(samples);
        if let Some(w) = self.writer.as_mut()
            && let Err(e) = w.write_samples(&pcm)
        {
            tracing::error!("opus encode failed: {e}");
        }
        self.sink.take()
    }

    fn finish(&mut self) -> Vec<u8> {
        let Some(mut writer) = self.writer.take() else {
            return Vec::new();
        };
        // Drain the resampler so no audio at the end of the stream is lost.
        if let Some(rs) = self.resampler.as_mut() {
            let tail = rs.flush();
            if !tail.is_empty()
                && let Err(e) = writer.write_samples(&tail)
            {
                tracing::error!("opus tail encode failed: {e}");
            }
        }
        if let Err(e) = writer.finish() {
            tracing::error!("opus finish failed: {e}");
        }
        self.sink.take()
    }

    fn reset(&mut self) {
        if let Err(e) = self.start_stream() {
            tracing::error!("opus encoder reset failed: {e}");
            self.writer = None;
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::SymphoniaDecoder;

    fn fmt(rate: u32, channels: u8) -> AudioFormat {
        AudioFormat {
            sample_rate: rate,
            channels,
            bits_per_sample: 16,
        }
    }

    /// Interleaved sine of `secs` seconds at `rate`; the same tone on every channel.
    fn sine(rate: u32, channels: usize, secs: f64, freq: f64, amp: f32) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        let mut v = Vec::with_capacity(n * channels);
        for i in 0..n {
            let s =
                amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    /// Split an Ogg byte stream into `(header_type, granule, serial, body)` pages.
    fn pages(mut d: &[u8]) -> Vec<(u8, i64, u32, Vec<u8>)> {
        let mut out = Vec::new();
        while !d.is_empty() {
            assert_eq!(&d[..4], b"OggS", "page capture pattern");
            let nseg = usize::from(d[26]);
            let body: usize = d[27..27 + nseg].iter().map(|&b| usize::from(b)).sum();
            let granule = i64::from_le_bytes(d[6..14].try_into().unwrap());
            let serial = u32::from_le_bytes(d[14..18].try_into().unwrap());
            out.push((
                d[5],
                granule,
                serial,
                d[27 + nseg..27 + nseg + body].to_vec(),
            ));
            d = &d[27 + nseg + body..];
        }
        out
    }

    /// Encode `pcm` in 1000-frame chunks and return header + audio + EOS bytes.
    fn encode_stream(enc: &mut OpusEncoder, pcm: &[f32], channels: usize) -> Vec<u8> {
        let mut bytes = enc.header();
        for chunk in pcm.chunks(1000 * channels) {
            bytes.extend(enc.encode(chunk));
        }
        bytes.extend(enc.finish());
        bytes
    }

    fn decode_bytes(bytes: &[u8]) -> (SymphoniaDecoder, Vec<f32>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.opus");
        std::fs::write(&path, bytes).unwrap();
        let mut dec = SymphoniaDecoder::open(&path).expect("open ogg opus");
        let mut all = Vec::new();
        let mut buf = vec![0.0f32; 4096];
        loop {
            let n = dec.read(&mut buf).expect("decode");
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        (dec, all)
    }

    #[test]
    fn header_is_opushead_and_opustags_pages() {
        let enc = OpusEncoder::new(fmt(44100, 2), OpusSettings::default()).unwrap();
        let h = enc.header();
        assert_eq!(&h[..4], b"OggS");
        let p = pages(&h);
        assert_eq!(p.len(), 2);
        assert_ne!(p[0].0 & 0x02, 0, "first page has BOS flag");
        assert_eq!(&p[0].3[..8], b"OpusHead");
        assert_eq!(p[0].3[9], 2, "stereo");
        assert_eq!(&p[1].3[..8], b"OpusTags");
        assert_eq!(enc.content_type(), "audio/ogg");
    }

    #[test]
    fn output_format_reports_48k_and_encoded_channels() {
        let enc = OpusEncoder::new(fmt(44100, 6), OpusSettings::default()).unwrap();
        let out = enc.output_format(fmt(44100, 6));
        assert_eq!((out.sample_rate, out.channels), (48000, 2));
        let enc = OpusEncoder::new(fmt(96000, 1), OpusSettings::default()).unwrap();
        let out = enc.output_format(fmt(96000, 1));
        assert_eq!((out.sample_rate, out.channels), (48000, 1));
    }

    #[test]
    fn mono_header_declares_one_channel() {
        let enc = OpusEncoder::new(fmt(48000, 1), OpusSettings::default()).unwrap();
        assert_eq!(pages(&enc.header())[0].3[9], 1);
    }

    #[test]
    fn finish_emits_eos_page_with_trimming_granule() {
        let mut enc = OpusEncoder::new(fmt(48000, 2), OpusSettings::default()).unwrap();
        let pcm = sine(48000, 2, 1.0, 440.0, 0.5);
        let bytes = encode_stream(&mut enc, &pcm, 2);
        let p = pages(&bytes);
        let last = p.last().unwrap();
        assert_ne!(last.0 & 0x04, 0, "last page has EOS flag");
        assert!(
            p[..p.len() - 1].iter().all(|pg| pg.0 & 0x04 == 0),
            "only the last page is EOS"
        );
        // pre-skip + 1 s of samples.
        assert_eq!(last.1, 48_000 + 120);
        // A second finish() has nothing left to say.
        assert!(enc.finish().is_empty());
    }

    #[test]
    fn reset_starts_new_stream_with_new_serial() {
        let mut enc = OpusEncoder::new(fmt(48000, 2), OpusSettings::default()).unwrap();
        let h1 = enc.header();
        let _ = enc.encode(&sine(48000, 2, 0.2, 440.0, 0.5));
        let _ = enc.finish();
        enc.reset();
        let h2 = enc.header();
        let (s1, s2) = (pages(&h1)[0].2, pages(&h2)[0].2);
        assert_ne!(s1, s2, "new logical stream needs a new serial");
        // Audio pages of the new stream carry the new serial.
        let out = enc.encode(&sine(48000, 2, 1.0, 440.0, 0.5));
        let p = pages(&out);
        assert!(!p.is_empty());
        assert!(p.iter().all(|pg| pg.2 == s2));
    }

    #[test]
    fn round_trip_48k_matches_input() {
        let mut enc = OpusEncoder::new(fmt(48000, 2), OpusSettings::default()).unwrap();
        let pcm = sine(48000, 2, 2.0, 1000.0, 0.5);
        let bytes = encode_stream(&mut enc, &pcm, 2);
        let (dec, out) = decode_bytes(&bytes);
        assert_eq!(dec.sample_rate(), 48000);
        assert_eq!(dec.channels(), 2);
        let frames = out.len() / 2;
        assert!(
            (frames as i64 - 96_000).abs() <= 960,
            "decoded {frames} frames, expected ~96000"
        );
        // Best SNR over a small alignment search, ignoring the edges.
        let mut best = f64::MIN;
        for lag in 0..=480usize {
            let (mut sig, mut err) = (0.0f64, 0.0f64);
            for i in 9_600..86_400usize {
                let r = f64::from(pcm[i * 2]);
                let d = f64::from(out[(i + lag) * 2]);
                sig += r * r;
                err += (r - d) * (r - d);
            }
            best = best.max(10.0 * (sig / err.max(1e-30)).log10());
        }
        assert!(best > 15.0, "SNR {best:.1} dB too low");
    }

    #[test]
    fn round_trip_44k1_resampled_duration_level_and_pitch() {
        let mut enc = OpusEncoder::new(fmt(44100, 2), OpusSettings::default()).unwrap();
        let pcm = sine(44100, 2, 3.0, 1000.0, 0.5);
        let bytes = encode_stream(&mut enc, &pcm, 2);
        let (dec, out) = decode_bytes(&bytes);
        assert_eq!(dec.sample_rate(), 48000);
        let frames = out.len() / 2;
        // Exactly 3 s at 48 kHz (the resampler tail is flushed on finish).
        assert!(
            (frames as i64 - 144_000).abs() <= 960,
            "decoded {frames} frames, expected ~144000"
        );
        // Steady-state RMS of a 0.5-amplitude sine is ~0.354.
        let mid: Vec<f32> = out[2 * 48_000..2 * 96_000]
            .iter()
            .step_by(2)
            .copied()
            .collect();
        let rms = (mid.iter().map(|s| f64::from(s * s)).sum::<f64>() / mid.len() as f64).sqrt();
        assert!((rms - 0.3536).abs() < 0.04, "rms {rms}");
        // 1 kHz tone => ~2000 zero crossings per second.
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!(
            (1900..=2100).contains(&crossings),
            "zero crossings {crossings}"
        );
    }

    #[test]
    fn six_channels_fold_to_stereo() {
        let mut enc = OpusEncoder::new(fmt(48000, 6), OpusSettings::default()).unwrap();
        assert_eq!(pages(&enc.header())[0].3[9], 2);
        let pcm = sine(48000, 6, 0.5, 440.0, 0.5);
        let bytes = encode_stream(&mut enc, &pcm, 6);
        let (dec, out) = decode_bytes(&bytes);
        assert_eq!(dec.channels(), 2);
        assert!(out.iter().any(|s| s.abs() > 0.05));
    }

    #[test]
    fn downmix_weights_cannot_clip() {
        for n in 3..=8 {
            let w = downmix_weights(n);
            assert!(w.iter().map(|p| p.0).sum::<f32>() <= 1.0 + 1e-6, "{n}ch L");
            assert!(w.iter().map(|p| p.1).sum::<f32>() <= 1.0 + 1e-6, "{n}ch R");
        }
    }

    #[test]
    fn cbr_packets_are_constant_size_and_bitrate_is_respected() {
        let settings = OpusSettings::default()
            .with_bitrate_kbps(64.0)
            .with_complexity(5.0);
        let settings = OpusSettings {
            mode: BitrateMode::Cbr,
            ..settings
        };
        let mut enc = OpusEncoder::new(fmt(48000, 2), settings).unwrap();
        let bytes = encode_stream(&mut enc, &sine(48000, 2, 4.0, 440.0, 0.5), 2);
        // 4 s at 64 kbit/s = 32 000 bytes of packets (+ Ogg framing).
        let audio: usize = pages(&bytes)[2..].iter().map(|p| p.3.len()).sum();
        assert!(
            (30_000..=36_000).contains(&audio),
            "audio payload {audio} bytes"
        );
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(parse_mode("VBR").unwrap(), BitrateMode::Vbr);
        assert_eq!(parse_mode("cvbr").unwrap(), BitrateMode::ConstrainedVbr);
        assert_eq!(parse_mode("cbr").unwrap(), BitrateMode::Cbr);
        assert_eq!(parse_mode("false").unwrap(), BitrateMode::Cbr);
        assert!(parse_mode("abr").is_err());
    }

    #[test]
    fn settings_are_clamped() {
        let s = OpusSettings::default()
            .with_bitrate_kbps(1.0)
            .with_complexity(99.0);
        assert_eq!(s.bitrate, MIN_BITRATE);
        assert_eq!(s.complexity, MAX_COMPLEXITY);
        let s = OpusSettings::default().with_bitrate_kbps(100_000.0);
        assert_eq!(s.bitrate, MAX_BITRATE);
    }

    #[test]
    fn rejects_zero_channels() {
        assert!(OpusEncoder::new(fmt(48000, 0), OpusSettings::default()).is_err());
    }
}
