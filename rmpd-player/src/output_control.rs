//! Lock-free, allocation-free control block shared between the engine's
//! async API and every audio output's real-time callback.
//!
//! Pause / flush / gain must reach the audio thread instantly, without
//! waiting for the decode thread to notice on its next loop turn — that is
//! the whole point of this type. Every field is a plain atomic; there is no
//! mutex and no allocation on the hot path (methods here are called from the
//! cpal / PipeWire / DoP real-time callback on every sample).
//!
//! One [`OutputControl`] is owned by the [`crate::engine::PlaybackEngine`]
//! for its entire lifetime (not per-song, not per-`MultiOutput`) and cloned
//! into every backend constructed for it. That way `pause()` / `seek()` /
//! `set_volume()` act on the SAME control block the real-time callback reads,
//! regardless of whether `OutputSlot` rebuilt the output or reused a cached
//! one (e.g. gapless `next`/`previous` reusing the same open device).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Shared out-of-band control state for one engine's audio outputs.
///
/// `flush_generation` is bumped by the engine BEFORE a seek, stop, or
/// user-initiated song change (see [`Self::flush`]). Every audio chunk
/// produced after that point is tagged with the new generation; every
/// backend drops chunks — and any in-flight partial chunk — whose
/// generation is stale. Natural gapless / crossfade transitions do NOT bump
/// it, so they are never interrupted.
#[derive(Debug)]
pub struct OutputControl {
    paused: AtomicBool,
    flush_generation: AtomicU64,
    /// Linear gain (0.0..=1.0 in normal use), IEEE-754 bits of an `f32`.
    /// Applied at playback time in the callback with a short ramp to avoid
    /// clicks, not baked into chunks at decode/write time.
    gain_bits: AtomicU32,
    /// Frames (not interleaved samples) actually handed to the device since
    /// the last [`Self::flush`] or [`Self::reset_played_frames`]. Used to
    /// compute the audible playback position, immune to however deep the
    /// upstream queues are.
    played_frames: AtomicU64,
}

impl OutputControl {
    pub fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            flush_generation: AtomicU64::new(0),
            gain_bits: AtomicU32::new(1.0f32.to_bits()),
            played_frames: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    #[inline]
    pub fn generation(&self) -> u64 {
        self.flush_generation.load(Ordering::Acquire)
    }

    /// Bump the flush generation and reset the played-frame counter.
    ///
    /// Call this BEFORE issuing a seek, a user-initiated stop, or a
    /// user-initiated song change (next/previous/play N/clear-while-playing).
    /// Every backend drops queued and in-flight audio tagged with an older
    /// generation, so stale audio never reaches the device. Returns the new
    /// generation.
    pub fn flush(&self) -> u64 {
        let g = self.flush_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.played_frames.store(0, Ordering::Release);
        g
    }

    #[inline]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Acquire))
    }

    #[inline]
    pub fn set_gain(&self, gain: f32) {
        self.gain_bits.store(gain.to_bits(), Ordering::Release);
    }

    #[inline]
    pub fn played_frames(&self) -> u64 {
        self.played_frames.load(Ordering::Acquire)
    }

    #[inline]
    pub fn add_played_frames(&self, n: u64) {
        self.played_frames.fetch_add(n, Ordering::AcqRel);
    }

    /// Reset the played-frame counter WITHOUT bumping the flush generation.
    ///
    /// Used at natural (non-flushing) song boundaries — gapless / crossfade
    /// in-thread advances — so `elapsed` restarts at 0 for the new song
    /// without discarding any already-queued audio (which a `flush()` would
    /// do, audibly interrupting the transition).
    #[inline]
    pub fn reset_played_frames(&self) {
        self.played_frames.store(0, Ordering::Release);
    }
}

impl Default for OutputControl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_defaults_false_and_round_trips() {
        let c = OutputControl::new();
        assert!(!c.is_paused());
        c.set_paused(true);
        assert!(c.is_paused());
        c.set_paused(false);
        assert!(!c.is_paused());
    }

    #[test]
    fn gain_defaults_to_unity_and_round_trips() {
        let c = OutputControl::new();
        assert!((c.gain() - 1.0).abs() < f32::EPSILON);
        c.set_gain(0.25);
        assert!((c.gain() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn flush_bumps_generation_monotonically() {
        let c = OutputControl::new();
        assert_eq!(c.generation(), 0);
        assert_eq!(c.flush(), 1);
        assert_eq!(c.flush(), 2);
        assert_eq!(c.generation(), 2);
    }

    #[test]
    fn flush_resets_played_frames() {
        let c = OutputControl::new();
        c.add_played_frames(1_000);
        assert_eq!(c.played_frames(), 1_000);
        c.flush();
        assert_eq!(c.played_frames(), 0);
    }

    #[test]
    fn played_frames_accumulate() {
        let c = OutputControl::new();
        c.add_played_frames(10);
        c.add_played_frames(5);
        assert_eq!(c.played_frames(), 15);
    }

    #[test]
    fn reset_played_frames_does_not_bump_generation() {
        let c = OutputControl::new();
        c.flush();
        let gen_before = c.generation();
        c.add_played_frames(42);
        c.reset_played_frames();
        assert_eq!(c.played_frames(), 0);
        assert_eq!(
            c.generation(),
            gen_before,
            "a soft reset (natural song transition) must not flush queued audio"
        );
    }
}
