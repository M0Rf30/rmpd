// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

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

use parking_lot::Mutex;
use std::sync::Arc;
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
    shared: Arc<SharedState>,
    /// Linear gain (0.0..=1.0 in normal use), IEEE-754 bits of an `f32`.
    /// Applied at playback time in the callback with a short ramp to avoid
    /// clicks, not baked into chunks at decode/write time. Per-control (see
    /// [`Self::with_own_gain`]) so every output can carry its own gain.
    gain_bits: AtomicU32,
}

/// State shared by an engine's control block and every per-output derivative.
#[derive(Debug)]
struct SharedState {
    paused: AtomicBool,
    flush_generation: AtomicU64,
    /// Frames (not interleaved samples) actually handed to the device since
    /// the last [`OutputControl::flush`] or [`OutputControl::reset_played_frames`].
    /// Used to compute the audible playback position, immune to however deep
    /// the upstream queues are.
    played_frames: AtomicU64,
}

impl OutputControl {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(SharedState {
                paused: AtomicBool::new(false),
                flush_generation: AtomicU64::new(0),
                played_frames: AtomicU64::new(0),
            }),
            gain_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    /// A control block sharing pause / flush / played-frames state with
    /// `self` but with its OWN gain, initialised to `gain`. Lets each output
    /// apply (or skip) the software volume independently.
    pub fn with_own_gain(&self, gain: f32) -> Self {
        Self {
            shared: self.shared.clone(),
            gain_bits: AtomicU32::new(gain.to_bits()),
        }
    }

    #[inline]
    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }

    #[inline]
    pub fn generation(&self) -> u64 {
        self.shared.flush_generation.load(Ordering::Acquire)
    }

    /// Bump the flush generation and reset the played-frame counter.
    ///
    /// Call this BEFORE issuing a seek, a user-initiated stop, or a
    /// user-initiated song change (next/previous/play N/clear-while-playing).
    /// Every backend drops queued and in-flight audio tagged with an older
    /// generation, so stale audio never reaches the device. Returns the new
    /// generation.
    pub fn flush(&self) -> u64 {
        let g = self.shared.flush_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.played_frames.store(0, Ordering::Release);
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
        self.shared.played_frames.load(Ordering::Acquire)
    }

    #[inline]
    pub fn add_played_frames(&self, n: u64) {
        self.shared.played_frames.fetch_add(n, Ordering::AcqRel);
    }

    /// Reset the played-frame counter WITHOUT bumping the flush generation.
    ///
    /// Used at natural (non-flushing) song boundaries — gapless / crossfade
    /// in-thread advances — so `elapsed` restarts at 0 for the new song
    /// without discarding any already-queued audio (which a `flush()` would
    /// do, audibly interrupting the transition).
    #[inline]
    pub fn reset_played_frames(&self) {
        self.shared.played_frames.store(0, Ordering::Release);
    }
}

/// Per-output software gain slots of one engine.
///
/// Every output gets a control block (see [`OutputControl::with_own_gain`])
/// sharing pause/flush/played-frames with the engine but with its own gain:
/// outputs on the software mixer follow the requested volume, hardware /
/// `none`-mixer outputs stay at unity so the device mixer is the only
/// attenuation.
#[derive(Debug, Default)]
pub struct OutputGains {
    slots: Mutex<Vec<GainSlot>>,
}

#[derive(Debug)]
struct GainSlot {
    software: bool,
    control: Arc<OutputControl>,
}

impl OutputGains {
    /// Forget all registered outputs (call before (re)building the outputs).
    pub fn clear(&self) {
        self.slots.lock().clear();
    }

    /// Create and register the control block for one output. A software
    /// output is seeded with the master's current gain, read while holding
    /// the slots lock so a concurrent [`Self::set_software_gain`] (which
    /// sets the master first, then takes the lock) can never be missed.
    pub fn register(&self, master: &OutputControl, software: bool) -> Arc<OutputControl> {
        let mut slots = self.slots.lock();
        let control = Arc::new(master.with_own_gain(if software { master.gain() } else { 1.0 }));
        slots.push(GainSlot {
            software,
            control: control.clone(),
        });
        control
    }

    /// Set the software gain of every software-mixer output.
    pub fn set_software_gain(&self, gain: f32) {
        for slot in self.slots.lock().iter().filter(|s| s.software) {
            slot.control.set_gain(gain);
        }
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

    #[test]
    fn per_output_gain_shares_state_but_not_gain() {
        let master = OutputControl::new();
        let child = master.with_own_gain(0.5);
        assert!((child.gain() - 0.5).abs() < f32::EPSILON);
        master.set_gain(0.2);
        assert!((child.gain() - 0.5).abs() < f32::EPSILON);
        master.set_paused(true);
        assert!(child.is_paused());
        child.add_played_frames(7);
        assert_eq!(master.played_frames(), 7);
        master.flush();
        assert_eq!(child.generation(), 1);
    }

    #[test]
    fn output_gains_only_move_software_outputs() {
        let master = OutputControl::new();
        master.set_gain(0.4);
        let gains = OutputGains::default();
        let soft = gains.register(&master, true);
        let hard = gains.register(&master, false);
        assert!((soft.gain() - 0.4).abs() < f32::EPSILON);
        assert!((hard.gain() - 1.0).abs() < f32::EPSILON);
        gains.set_software_gain(0.8);
        assert!((soft.gain() - 0.8).abs() < f32::EPSILON);
        assert!((hard.gain() - 1.0).abs() < f32::EPSILON);
    }
}
