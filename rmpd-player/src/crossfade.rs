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

/// Port of MPD's `mixramp_interpolate`: scan a MixRamp ramp list
/// `"<db> <sec>;<db> <sec>;..."` (dB values monotonically increasing) and
/// return the time in seconds at which the level crosses `required_db`.
///
/// Returns `None` when the required level is above all entries, or on any
/// parse failure (MPD returns −1; we use `None` so callers fall back cleanly).
#[must_use]
pub fn mixramp_interpolate(ramp_list: &str, required_db: f32) -> Option<f32> {
    let mut last_db: f32 = 0.0;
    let mut last_dur: f32 = 0.0;
    let mut has_last = false;

    for entry in ramp_list.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (db_str, dur_str) = entry.split_once(' ')?;
        let db: f32 = db_str.trim().parse().ok()?;
        let dur: f32 = dur_str.trim().parse().ok()?;

        if db == required_db {
            return Some(dur);
        }
        if db < required_db {
            last_db = db;
            last_dur = dur;
            has_last = true;
        } else {
            // db > required_db
            if !has_last {
                // required is below all entries: return the first (smallest) time
                return Some(dur);
            }
            // interpolate between the last-saved point and this one
            return Some(last_dur + (required_db - last_db) * (dur - last_dur) / (db - last_db));
        }
    }
    // required_db is above all entries → no crossing found
    None
}

/// Compute the MixRamp crossfade overlap window in seconds, or `None` to fall
/// back to a time-based crossfade.
///
/// * `next_start` — the incoming track's `MIXRAMP_START` tag value.
/// * `cur_end`    — the outgoing track's `MIXRAMP_END` tag value.
/// * `*_rg_db`   — ReplayGain dB already applied to each respective track.
/// * `delay`     — extra silence gap from `mixrampdelay` (seconds).
#[must_use]
pub fn mixramp_overlap_seconds(
    next_start: Option<&str>,
    cur_end: Option<&str>,
    mixramp_db: f32,
    cur_rg_db: f32,
    next_rg_db: f32,
    delay: f32,
) -> Option<f32> {
    let (ns, ce) = (next_start?, cur_end?);
    let oc = mixramp_interpolate(ns, mixramp_db - next_rg_db)?;
    let op = mixramp_interpolate(ce, mixramp_db - cur_rg_db)?;
    if oc < 0.0 || op < 0.0 {
        return None;
    }
    let overlap = oc + op;
    if delay <= overlap {
        Some(overlap - delay)
    } else {
        None
    }
}

/// Interleaved-sample count for a fractional-seconds window.
#[must_use]
pub fn window_samples_secs(sample_rate: u32, channels: u8, seconds: f32) -> usize {
    ((sample_rate as f32 * channels as f32 * seconds).max(0.0)) as usize
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

    // ── mixramp_interpolate ───────────────────────────────────────────────

    #[test]
    fn mixramp_exact_match() {
        // dB value is exactly in the list → return its time directly.
        assert_eq!(mixramp_interpolate("-10 1.5;-5 3.0", -10.0), Some(1.5));
        assert_eq!(mixramp_interpolate("-10 1.5;-5 3.0", -5.0), Some(3.0));
    }

    #[test]
    fn mixramp_interpolates_between_points() {
        // list: -10 @ 0.0 s, -5 @ 2.0 s.  required = -7.5 → midpoint → 1.0 s
        assert!(close(
            mixramp_interpolate("-10 0.0;-5 2.0", -7.5).unwrap(),
            1.0
        ));
    }

    #[test]
    fn mixramp_required_below_all_returns_first() {
        // required_db is below the first entry → MPD returns that entry's time
        // (the "least" crossing).
        assert_eq!(mixramp_interpolate("-5 1.0;-3 2.0", -10.0), Some(1.0));
    }

    #[test]
    fn mixramp_required_above_all_returns_none() {
        // required_db is higher than every entry → no crossing found.
        assert_eq!(mixramp_interpolate("-10 0.0;-5 2.0", 0.0), None);
    }

    #[test]
    fn mixramp_malformed_returns_none() {
        assert_eq!(mixramp_interpolate("notadb 1.0", -5.0), None);
        assert_eq!(mixramp_interpolate("-5", -5.0), None); // missing seconds
    }

    #[test]
    fn mixramp_empty_list_returns_none() {
        assert_eq!(mixramp_interpolate("", -5.0), None);
        assert_eq!(mixramp_interpolate(";;;", -5.0), None);
    }

    // ── mixramp_overlap_seconds ───────────────────────────────────────────

    #[test]
    fn overlap_none_when_tag_missing() {
        // Either tag missing → None.
        assert_eq!(
            mixramp_overlap_seconds(None, Some("-10 2.0"), -17.0, 0.0, 0.0, 0.0),
            None
        );
        assert_eq!(
            mixramp_overlap_seconds(Some("-10 2.0"), None, -17.0, 0.0, 0.0, 0.0),
            None
        );
    }

    #[test]
    fn overlap_sum_minus_delay() {
        // next_start "-20 0.0;-15 2.0", required -17 → (−17−−20)·2.0/(−15−−20) = 1.2 s
        // cur_end    "-20 0.0;-15 3.0", required -17 → 3·3.0/5 = 1.8 s
        let start = "-20 0.0;-15 2.0";
        let end = "-20 0.0;-15 3.0";
        let expected = 1.2 + 1.8; // = 3.0 (delay 0)
        let got = mixramp_overlap_seconds(Some(start), Some(end), -17.0, 0.0, 0.0, 0.0);
        assert!(close(got.unwrap(), expected));
    }

    #[test]
    fn overlap_none_when_delay_exceeds_overlap() {
        // overlap = 1.2 + 1.8 = 3.0 s, delay = 4.0 s → None
        let start = "-20 0.0;-15 2.0";
        let end = "-20 0.0;-15 3.0";
        assert_eq!(
            mixramp_overlap_seconds(Some(start), Some(end), -17.0, 0.0, 0.0, 4.0),
            None
        );
    }

    // ── window_samples_secs ───────────────────────────────────────────────

    #[test]
    fn window_samples_fractional() {
        // 0.5 s of 44100 Hz stereo = 44100 * 2 * 0.5 = 44100
        assert_eq!(window_samples_secs(44100, 2, 0.5), 44100);
        // Negative seconds → 0
        assert_eq!(window_samples_secs(44100, 2, -1.0), 0);
    }
}
