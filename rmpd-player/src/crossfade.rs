//! Crossfade / MixRamp DSP primitives.
//!
//! These are the pure, deterministic building blocks for overlapping two
//! tracks at a boundary: the gain curves, the buffer mixer, the window size,
//! and the MixRamp dB→gain conversion. They are decoupled from the playback
//! engine so they can be unit-tested without an audio device.
//!
//! Wiring these into playback requires decode look-ahead (opening the next
//! decoder before EOS so two streams overlap) and is an audible-tuning task;
//! see `docs/PLUGIN_ARCHITECTURE.md`. This module is that integration's
//! verified foundation.

use std::f32::consts::FRAC_PI_2;

/// Equal-power (constant-power) crossfade gains for a normalised `progress`
/// in `0.0..=1.0`. Returns `(fade_out, fade_in)`.
///
/// Equal-power keeps the *summed power* of the two streams roughly constant
/// across the fade (no perceived dip in the middle), which is why it is the
/// standard choice for music crossfades: `out = cos(p·π/2)`, `in = sin(p·π/2)`.
/// At `p = 0.5` both gains are ≈0.707 (−3 dB).
#[must_use]
pub fn equal_power_gains(progress: f32) -> (f32, f32) {
    let p = progress.clamp(0.0, 1.0);
    let angle = p * FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// Linear crossfade gains for `progress` in `0.0..=1.0`: `(1 - p, p)`.
///
/// Linear sums to constant *amplitude* (not power), so correlated material can
/// dip ~−6 dB mid-fade; prefer [`equal_power_gains`] for music.
#[must_use]
pub fn linear_gains(progress: f32) -> (f32, f32) {
    let p = progress.clamp(0.0, 1.0);
    (1.0 - p, p)
}

/// Number of interleaved f32 samples in a `seconds`-long window for the given
/// stream geometry (`sample_rate` Hz, `channels` channels).
#[must_use]
pub fn crossfade_window_samples(sample_rate: u32, channels: u8, seconds: u32) -> usize {
    sample_rate as usize * channels as usize * seconds as usize
}

/// Mix `src` into `dest` in place with per-stream gains:
/// `dest[i] = dest[i] * dest_gain + src[i] * src_gain`, over the overlapping
/// prefix (`min(dest.len(), src.len())`). Used to overlap the fading-out tail
/// of one track with the fading-in head of the next.
pub fn mix_into(dest: &mut [f32], src: &[f32], dest_gain: f32, src_gain: f32) {
    let n = dest.len().min(src.len());
    for i in 0..n {
        dest[i] = dest[i] * dest_gain + src[i] * src_gain;
    }
}

/// Convert a MixRamp threshold in dBFS to a linear amplitude (`10^(db/20)`).
///
/// MixRamp uses `mixrampdb` plus the per-file `MIXRAMP_START`/`MIXRAMP_END`
/// analysis tags to pick the overlap point where the outgoing track has decayed
/// to this level; computing that point needs the tags, but the dB→gain step is
/// shared and unit-testable here.
#[must_use]
pub fn mixramp_db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn equal_power_endpoints_and_midpoint() {
        let (o0, i0) = equal_power_gains(0.0);
        assert!(
            close(o0, 1.0) && close(i0, 0.0),
            "start: full out, silent in"
        );
        let (o1, i1) = equal_power_gains(1.0);
        assert!(close(o1, 0.0) && close(i1, 1.0), "end: silent out, full in");
        let (om, im) = equal_power_gains(0.5);
        let mid = std::f32::consts::FRAC_1_SQRT_2; // ≈0.7071, −3 dB
        assert!(close(om, mid) && close(im, mid), "−3 dB mid");
        // Equal power: gains² sum to 1 across the fade.
        for step in 0..=10 {
            let (o, i) = equal_power_gains(step as f32 / 10.0);
            assert!(close(o * o + i * i, 1.0), "constant power invariant");
        }
    }

    #[test]
    fn gains_clamp_out_of_range() {
        assert_eq!(equal_power_gains(-1.0), equal_power_gains(0.0));
        assert_eq!(equal_power_gains(2.0), equal_power_gains(1.0));
        assert_eq!(linear_gains(-0.5), (1.0, 0.0));
        assert_eq!(linear_gains(1.5), (0.0, 1.0));
    }

    #[test]
    fn linear_midpoint() {
        assert_eq!(linear_gains(0.5), (0.5, 0.5));
    }

    #[test]
    fn window_sample_count() {
        // 2 s of 44.1 kHz stereo = 44100 * 2 * 2.
        assert_eq!(crossfade_window_samples(44100, 2, 2), 176_400);
        assert_eq!(crossfade_window_samples(48000, 1, 0), 0);
    }

    #[test]
    fn mix_into_applies_gains_over_overlap() {
        let mut dest = [1.0f32, 1.0, 1.0, 1.0];
        let src = [0.5f32, 0.5]; // shorter: only the prefix is mixed
        mix_into(&mut dest, &src, 0.25, 2.0);
        assert!(close(dest[0], 0.25 * 1.0 + 2.0 * 0.5));
        assert!(close(dest[1], 0.25 * 1.0 + 2.0 * 0.5));
        // Beyond src.len() dest is untouched.
        assert!(close(dest[2], 1.0) && close(dest[3], 1.0));
    }

    #[test]
    fn mixramp_db_conversions() {
        assert!(close(mixramp_db_to_gain(0.0), 1.0));
        assert!(close(mixramp_db_to_gain(-6.0), 0.501_187_2));
        assert!(close(mixramp_db_to_gain(-20.0), 0.1));
    }
}
