//! Best-effort streaming sample-rate conversion.
//!
//! A small linear-interpolation resampler used only as a fallback when the
//! output device cannot natively play the decoded stream's sample rate (for
//! example a 48 kHz-only device handed a 44.1 kHz-family DSD-to-PCM stream).
//! Its job is to guarantee that audio plays in *any* case; when the device
//! supports the source rate natively, no resampler is created and samples pass
//! through untouched.
//!
//! Linear interpolation is intentionally simple and cheap. It is more than
//! adequate for this last-resort path; higher-quality conversion (e.g. rubato)
//! can be substituted later without changing callers.

/// Streaming linear-interpolation resampler for interleaved `f32` audio.
pub struct LinearResampler {
    channels: usize,
    /// Input frames consumed per output frame (`src_rate / dst_rate`).
    step: f64,
    /// Position of the next output sample, in input frames, relative to the
    /// first frame of the current `process` block. May be in `[-1, 0)`, in
    /// which case it interpolates from `prev` (the previous block's last frame).
    pos: f64,
    /// Last input frame of the previous block (one sample per channel).
    prev: Vec<f32>,
}

impl LinearResampler {
    /// Create a resampler converting from `src_rate` to `dst_rate` for the
    /// given (interleaved) channel count.
    pub fn new(src_rate: u32, dst_rate: u32, channels: usize) -> Self {
        let dst = dst_rate.max(1);
        Self {
            channels,
            step: src_rate as f64 / dst as f64,
            pos: 0.0,
            prev: vec![0.0; channels.max(1)],
        }
    }

    /// Resample one block of interleaved input, returning interleaved output at
    /// the destination rate. Internal state is carried across calls so block
    /// boundaries interpolate continuously.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        if ch == 0 || input.len() < ch {
            return Vec::new();
        }

        let in_frames = input.len() / ch;
        let mut out = Vec::with_capacity(((in_frames as f64 / self.step) as usize + 1) * ch);

        // We can interpolate while the upper neighbour (floor(pos) + 1) is still
        // within this block, i.e. while pos < in_frames - 1.
        let upper = in_frames as f64 - 1.0;
        while self.pos < upper {
            let floor = self.pos.floor();
            let idx = floor as isize; // always >= -1
            let frac = (self.pos - floor) as f32;
            for c in 0..ch {
                let a = if idx < 0 {
                    self.prev[c]
                } else {
                    input[idx as usize * ch + c]
                };
                let b = input[(idx + 1) as usize * ch + c];
                out.push(a + (b - a) * frac);
            }
            self.pos += self.step;
        }

        // Carry the last input frame and rebase the position so the next block's
        // frame 0 follows this block's last frame. `pos` lands in `[-1, ..)`.
        let base = (in_frames - 1) * ch;
        self.prev.copy_from_slice(&input[base..base + ch]);
        self.pos -= in_frames as f64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_ratio_is_one() {
        let mut r = LinearResampler::new(48000, 48000, 1);
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let out = r.process(&input);
        assert!((out.len() as i32 - input.len() as i32).abs() <= 1);
    }

    #[test]
    fn downsample_produces_fewer_samples() {
        let mut r = LinearResampler::new(88200, 44100, 1);
        let input: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let out = r.process(&input);
        assert!(out.len() >= 490 && out.len() <= 510, "got {}", out.len());
    }

    #[test]
    fn upsample_produces_more_samples_and_interpolates() {
        let mut r = LinearResampler::new(44100, 48000, 1);
        let input: Vec<f32> = (0..441).map(|i| i as f32).collect();
        let out = r.process(&input);
        assert!(out.len() > input.len());
        assert!(out.windows(2).all(|w| w[1] >= w[0] - 1e-3));
    }

    #[test]
    fn stereo_keeps_channel_separation() {
        let mut r = LinearResampler::new(96000, 48000, 2);
        let mut input = Vec::new();
        for _ in 0..200 {
            input.push(1.0f32);
            input.push(-1.0f32);
        }
        let out = r.process(&input);
        assert_eq!(out.len() % 2, 0);
        for frame in out.chunks(2) {
            assert!((frame[0] - 1.0).abs() < 1e-3, "ch0={}", frame[0]);
            assert!((frame[1] + 1.0).abs() < 1e-3, "ch1={}", frame[1]);
        }
    }

    #[test]
    fn continuous_across_blocks() {
        let whole: Vec<f32> = (0..2000).map(|i| i as f32).collect();
        let mut r1 = LinearResampler::new(88200, 48000, 1);
        let single = r1.process(&whole).len();

        let mut r2 = LinearResampler::new(88200, 48000, 1);
        let a = r2.process(&whole[..1000]).len();
        let b = r2.process(&whole[1000..]).len();
        assert!((single as i32 - (a + b) as i32).abs() <= 2);
    }
}
