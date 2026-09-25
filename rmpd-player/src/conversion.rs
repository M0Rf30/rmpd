//! Shared sample format conversion utilities for audio output backends.

use crate::output_control::OutputControl;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Convert f32 samples to s16le bytes, writing into the provided buffer.
/// The buffer is cleared and filled with the converted bytes.
pub fn samples_to_s16le_into(samples: &[f32], buf: &mut Vec<u8>) {
    buf.clear();
    buf.reserve(samples.len() * 2);
    for &s in samples {
        let v = f32_to_i16(s);
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// Convert interleaved f32 PCM samples (range −1.0…+1.0) to little-endian
/// signed 16-bit bytes.
pub fn samples_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(samples.len() * 2);
    samples_to_s16le_into(samples, &mut buf);
    buf
}

/// Clamp and scale a single f32 sample to `i16` range.
#[inline]
pub fn f32_to_i16(val: f32) -> i16 {
    (val.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Clamp and scale a single f32 sample to `i32` range.
#[inline]
pub fn f32_to_i32(val: f32) -> i32 {
    (val.clamp(-1.0, 1.0) * i32::MAX as f32) as i32
}

/// A chunk of interleaved samples tagged with the [`OutputControl`] flush
/// generation it was produced under.
///
/// Every backend's real-time callback compares this against the live
/// generation and drops the chunk (or the still-unconsumed tail of it) the
/// instant they diverge, rather than playing out stale audio.
pub struct Chunk<T> {
    pub generation: u64,
    pub samples: Vec<T>,
}

impl<T> Chunk<T> {
    pub fn new(generation: u64, samples: Vec<T>) -> Self {
        Self {
            generation,
            samples,
        }
    }
}

/// A bounded sample buffer fed from a `SyncSender`/`Receiver` channel,
/// pause- and flush-generation-aware via a shared [`OutputControl`].
///
/// Used inside output real-time callbacks (cpal, PipeWire) and the DoP
/// callback to decouple the decoder thread from the real-time audio thread.
///
/// * **Pause**: while `control.is_paused()`, [`Self::next_sample`] returns
///   silence WITHOUT touching the current buffer position or draining the
///   channel — so resume continues exactly where audio stopped, matching MPD.
/// * **Flush**: every sample compares the in-hand buffer's generation (and
///   any freshly-received chunk's generation) against `control.generation()`;
///   a stale buffer is dropped instantly (not played to its end) and stale
///   queued chunks are skipped without stalling.
/// * **Accounting**: every real (non-silence, non-paused) sample advances a
///   frame counter; once a full frame (all channels) has been emitted,
///   `control.played_frames()` is incremented — the basis for audible-elapsed
///   reporting.
pub struct SampleBuffer<T> {
    rx: Receiver<Chunk<T>>,
    control: Arc<OutputControl>,
    buffer: Vec<T>,
    buffer_generation: u64,
    pos: usize,
    channels: usize,
    frame_phase: usize,
}

impl<T: Default + Copy> SampleBuffer<T> {
    /// Create a new buffer backed by the receiving end of a `sync_channel`.
    /// `channels` is used only to turn consumed samples into played FRAMES
    /// for `control.played_frames()` accounting.
    pub fn new(rx: Receiver<Chunk<T>>, control: Arc<OutputControl>, channels: usize) -> Self {
        Self {
            rx,
            control,
            buffer: Vec::new(),
            buffer_generation: 0,
            pos: 0,
            channels: channels.max(1),
            frame_phase: 0,
        }
    }

    /// Return the next sample, refilling from the channel when the current
    /// chunk is exhausted. Returns `T::default()` (silence) while paused, on
    /// underrun, or immediately after dropping a stale-generation chunk with
    /// nothing fresh yet queued.
    #[inline]
    pub fn next_sample(&mut self) -> T {
        if self.control.is_paused() {
            // Hold position: don't touch buffer/pos, don't drain the
            // channel. Resume picks up exactly here.
            return T::default();
        }

        let generation = self.control.generation();

        // The in-hand buffer was produced under an older generation (a
        // flush happened mid-chunk) — drop the unconsumed tail instantly
        // instead of playing it out.
        if self.pos < self.buffer.len() && self.buffer_generation != generation {
            self.pos = self.buffer.len();
        }

        while self.pos >= self.buffer.len() {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    if chunk.generation != generation {
                        // Stale chunk queued before the flush: drop and
                        // keep looking without stalling the callback.
                        continue;
                    }
                    self.buffer = chunk.samples;
                    self.buffer_generation = chunk.generation;
                    self.pos = 0;
                }
                Err(_) => return T::default(), // underrun -> silence
            }
        }

        let val = self.buffer[self.pos];
        self.pos += 1;

        self.frame_phase += 1;
        if self.frame_phase >= self.channels {
            self.frame_phase = 0;
            self.control.add_played_frames(1);
        }

        val
    }
}

/// Ramps a live gain target smoothly over a few milliseconds so a volume
/// change never clicks. Operates purely in the f32 domain; callers quantize
/// afterwards for integer output formats.
pub struct GainRamp {
    current: f32,
    /// Maximum change in gain per output (interleaved) sample.
    step: f32,
}

/// Ramp duration: long enough to silence a click, short enough that a
/// volume command is inaudibly close to instant.
const RAMP_MS: f32 = 5.0;

impl GainRamp {
    /// `sample_rate` is the output device rate; `channels` the interleaved
    /// channel count (the ramp step is expressed per interleaved sample).
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let frames = (sample_rate.max(1) as f32) * (RAMP_MS / 1000.0);
        let interleaved = (frames * channels.max(1) as f32).max(1.0);
        Self {
            current: 1.0,
            step: 1.0 / interleaved,
        }
    }

    /// Advance the ramp one (interleaved) sample toward `target` and return
    /// `raw * ramped_gain`.
    #[inline]
    pub fn apply(&mut self, raw: f32, target: f32) -> f32 {
        if self.current < target {
            self.current = (self.current + self.step).min(target);
        } else if self.current > target {
            self.current = (self.current - self.step).max(target);
        }
        raw * self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn control() -> Arc<OutputControl> {
        Arc::new(OutputControl::new())
    }

    #[test]
    fn samples_to_s16le_clamps() {
        let samples = [0.0_f32, 1.0, -1.0, 1.5, -1.5];
        let bytes = samples_to_s16le(&samples);
        assert_eq!(bytes.len(), 10);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[6], bytes[7]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[8], bytes[9]]), -i16::MAX);
    }

    #[test]
    fn samples_to_s16le_into_reusable() {
        let samples = [0.0_f32, 1.0, -1.0, 1.5, -1.5];
        let mut buf = Vec::new();

        samples_to_s16le_into(&samples, &mut buf);
        assert_eq!(buf.len(), 10);
        assert_eq!(i16::from_le_bytes([buf[0], buf[1]]), 0);
        assert_eq!(i16::from_le_bytes([buf[2], buf[3]]), i16::MAX);

        let samples2 = [0.5_f32, -0.5];
        samples_to_s16le_into(&samples2, &mut buf);
        assert_eq!(buf.len(), 4);
        assert!(buf.capacity() >= 4);
    }

    #[test]
    fn f32_to_i16_basic() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert_eq!(f32_to_i16(-1.0), -i16::MAX);
    }

    #[test]
    fn f32_to_i32_basic() {
        assert_eq!(f32_to_i32(0.0), 0);
        assert!((f32_to_i32(1.0) - i32::MAX).unsigned_abs() < 256);
        assert!((f32_to_i32(-1.0) + i32::MAX).unsigned_abs() < 256);
    }

    #[test]
    fn sample_buffer_refill() {
        let (tx, rx) = sync_channel::<Chunk<f32>>(2);
        let ctl = control();
        let mut buf = SampleBuffer::new(rx, ctl, 1);

        tx.send(Chunk::new(0, vec![1.0, 2.0])).unwrap();
        tx.send(Chunk::new(0, vec![3.0])).unwrap();

        assert_eq!(buf.next_sample(), 1.0);
        assert_eq!(buf.next_sample(), 2.0);
        assert_eq!(buf.next_sample(), 3.0);
    }

    #[test]
    fn sample_buffer_underrun_returns_silence() {
        let (_tx, rx) = sync_channel::<Chunk<f32>>(1);
        let ctl = control();
        let mut buf = SampleBuffer::new(rx, ctl, 1);

        assert_eq!(buf.next_sample(), 0.0);
    }

    #[test]
    fn sample_buffer_i32_silence() {
        let (_tx, rx) = sync_channel::<Chunk<i32>>(1);
        let ctl = control();
        let mut buf: SampleBuffer<i32> = SampleBuffer::new(rx, ctl, 1);

        assert_eq!(buf.next_sample(), 0);
    }

    /// Pause holds position: no samples drain from the channel and no
    /// silence "eats into" the current buffer while paused; resume
    /// continues from exactly the same index.
    #[test]
    fn pause_holds_position_and_resume_continues_exactly() {
        let (tx, rx) = sync_channel::<Chunk<f32>>(4);
        let ctl = control();
        let mut buf = SampleBuffer::new(rx, Arc::clone(&ctl), 1);

        tx.send(Chunk::new(0, vec![1.0, 2.0, 3.0])).unwrap();
        assert_eq!(buf.next_sample(), 1.0);

        ctl.set_paused(true);
        for _ in 0..5 {
            assert_eq!(buf.next_sample(), 0.0, "paused output must be silence");
        }

        ctl.set_paused(false);
        // Must resume at sample index 1 (value 2.0), not skip ahead or
        // restart, and no extra chunk needed to be sent.
        assert_eq!(buf.next_sample(), 2.0);
        assert_eq!(buf.next_sample(), 3.0);
    }

    /// A flush (generation bump) drops both a partially-consumed in-hand
    /// buffer and any older-generation chunks still queued, without
    /// stalling on underrun.
    #[test]
    fn flush_drops_stale_partial_and_queued_chunks() {
        let (tx, rx) = sync_channel::<Chunk<f32>>(4);
        let ctl = control();
        let mut buf = SampleBuffer::new(rx, Arc::clone(&ctl), 1);

        // Queue two generation-0 chunks and consume one sample of the first.
        tx.send(Chunk::new(0, vec![9.0, 9.0, 9.0])).unwrap();
        tx.send(Chunk::new(0, vec![9.0, 9.0])).unwrap();
        assert_eq!(buf.next_sample(), 9.0);

        // Flush: bump the generation and enqueue fresh gen-1 audio.
        ctl.flush();
        tx.send(Chunk::new(1, vec![5.0, 6.0])).unwrap();

        // The remaining gen-0 tail and the whole second gen-0 chunk must be
        // dropped; the very next sample is the fresh gen-1 audio.
        assert_eq!(buf.next_sample(), 5.0);
        assert_eq!(buf.next_sample(), 6.0);
    }

    /// `played_frames` increments once per full frame (all channels), not
    /// per interleaved sample, and does not advance while paused or on
    /// underrun.
    #[test]
    fn played_frames_counts_frames_not_samples() {
        let (tx, rx) = sync_channel::<Chunk<f32>>(2);
        let ctl = control();
        let mut buf = SampleBuffer::new(rx, Arc::clone(&ctl), 2); // stereo

        tx.send(Chunk::new(0, vec![1.0, 2.0, 3.0, 4.0])).unwrap(); // 2 frames

        assert_eq!(buf.next_sample(), 1.0);
        assert_eq!(ctl.played_frames(), 0, "half a frame consumed so far");
        assert_eq!(buf.next_sample(), 2.0);
        assert_eq!(ctl.played_frames(), 1);
        assert_eq!(buf.next_sample(), 3.0);
        assert_eq!(buf.next_sample(), 4.0);
        assert_eq!(ctl.played_frames(), 2);

        // Underrun samples must not count as played.
        assert_eq!(buf.next_sample(), 0.0);
        assert_eq!(ctl.played_frames(), 2);

        // Nor must paused samples.
        ctl.set_paused(true);
        assert_eq!(buf.next_sample(), 0.0);
        assert_eq!(ctl.played_frames(), 2);
    }

    // ── GainRamp ─────────────────────────────────────────────────────────

    #[test]
    fn gain_ramp_reaches_target_and_applies_multiplicatively() {
        let mut ramp = GainRamp::new(1000, 1); // 1000 Hz -> 5 samples to ramp
        let mut out = 0.0;
        for _ in 0..100 {
            out = ramp.apply(1.0, 0.5);
        }
        assert!(
            (out - 0.5).abs() < 1e-6,
            "ramp must settle at target, got {out}"
        );
    }

    #[test]
    fn gain_ramp_does_not_jump_instantly() {
        let mut ramp = GainRamp::new(48_000, 2); // long ramp in sample terms
        let first = ramp.apply(1.0, 0.0);
        assert!(
            first > 0.9,
            "gain must not jump to the target on the very first sample (got {first})"
        );
    }
}
