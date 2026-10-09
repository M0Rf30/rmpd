// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Streaming sample-rate conversion.
//!
//! Used only as a fallback when the output device cannot natively play the
//! decoded stream's sample rate (for example a hardware-locked 48 kHz device
//! handed a 44.1 kHz-family DSD-to-PCM stream). When the device supports the
//! source rate natively no resampler is created and samples pass through
//! untouched.
//!
//! Backed by `rubato`'s asynchronous resampler. The sinc modes apply a real
//! anti-aliasing filter — essential when downsampling DSD-derived PCM, which
//! carries large ultrasonic shaped noise that would otherwise alias into the
//! audible band — while the `Linear` mode uses cheap polynomial interpolation
//! with no anti-aliasing.

use audioadapter_buffers::direct::InterleavedSlice;
use rmpd_core::config::ResamplerQuality;
use rubato::{
    Async, FixedAsync, Indexing, PolynomialDegree, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction, calculate_cutoff,
};

/// Number of input frames fed to the resampler per processing chunk. With a
/// fixed-input async resampler this is also `input_frames_next()`.
const CHUNK_FRAMES: usize = 1024;

/// Streaming, anti-aliased sample-rate converter for interleaved `f32` audio.
pub struct StreamResampler {
    resampler: Async<f32>,
    channels: usize,
    /// Input frames required per `process_into_buffer` call (constant for a
    /// fixed-input async resampler).
    chunk: usize,
    /// Interleaved accumulator of input samples not yet consumed.
    input: Vec<f32>,
    /// Interleaved scratch buffer holding one chunk of resampler output.
    scratch: Vec<f32>,
    /// Destination/source rate ratio.
    ratio: f64,
    /// Leading output frames (filter latency) still to be dropped so the
    /// output is time-aligned with the input.
    delay_left: usize,
    /// Input frames accepted since construction / the last [`Self::flush`].
    frames_in: u64,
    /// Output frames returned since construction / the last [`Self::flush`].
    frames_out: u64,
}

impl StreamResampler {
    /// Create a resampler converting interleaved `channels`-channel audio from
    /// `src_rate` to `dst_rate` at the requested `quality`.
    ///
    /// Returns `None` if the resampler could not be constructed; the caller
    /// should then fall back to passthrough.
    pub fn new(
        src_rate: u32,
        dst_rate: u32,
        channels: usize,
        quality: ResamplerQuality,
    ) -> Option<Self> {
        let channels = channels.max(1);
        let ratio = f64::from(dst_rate.max(1)) / f64::from(src_rate.max(1));

        let resampler = match quality {
            ResamplerQuality::Linear => Async::<f32>::new_poly(
                ratio,
                1.1,
                PolynomialDegree::Linear,
                CHUNK_FRAMES,
                channels,
                FixedAsync::Input,
            )
            .ok()?,
            _ => {
                let params = sinc_params(quality);
                Async::<f32>::new_sinc(
                    ratio,
                    1.1,
                    &params,
                    CHUNK_FRAMES,
                    channels,
                    FixedAsync::Input,
                )
                .ok()?
            }
        };

        let chunk = resampler.input_frames_next();
        let scratch = vec![0.0; resampler.output_frames_max() * channels];
        let delay_left = resampler.output_delay();

        Some(Self {
            resampler,
            channels,
            chunk,
            input: Vec::new(),
            scratch,
            ratio,
            delay_left,
            frames_in: 0,
            frames_out: 0,
        })
    }

    /// Resample one block of interleaved input, returning interleaved output at
    /// the destination rate. Leftover input (less than one chunk) is carried
    /// across calls so block boundaries stay continuous. The filter's leading
    /// latency is dropped, so the output is time-aligned with the input; call
    /// [`Self::flush`] at the end of a stream to get the remaining tail.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.frames_in += (input.len() / self.channels) as u64;
        self.input.extend_from_slice(input);
        self.drain_chunks()
    }

    /// Resample every complete chunk currently buffered in `self.input`.
    fn drain_chunks(&mut self) -> Vec<f32> {
        let ch = self.channels;
        let chunk_samples = self.chunk * ch;
        let mut out = Vec::new();

        while self.input.len() >= chunk_samples {
            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            // Borrow three disjoint fields (`input`, `scratch`, `resampler`)
            // inside a block so the adapters release them before we read the
            // output and drain the input below.
            let (nbr_in, nbr_out) = {
                let in_adapter =
                    match InterleavedSlice::new(&self.input[..chunk_samples], ch, self.chunk) {
                        Ok(a) => a,
                        Err(_) => break,
                    };
                let out_cap = self.scratch.len() / ch;
                let mut out_adapter =
                    match InterleavedSlice::new_mut(&mut self.scratch, ch, out_cap) {
                        Ok(a) => a,
                        Err(_) => break,
                    };
                match self.resampler.process_into_buffer(
                    &in_adapter,
                    &mut out_adapter,
                    Some(&indexing),
                ) {
                    Ok(counts) => counts,
                    Err(_) => break,
                }
            };

            // Guard against a pathological zero-consumption result that would
            // otherwise spin forever.
            if nbr_in == 0 {
                break;
            }

            // Drop the filter latency from the start of the stream.
            let skip = self.delay_left.min(nbr_out);
            self.delay_left -= skip;
            out.extend_from_slice(&self.scratch[skip * ch..nbr_out * ch]);
            self.frames_out += (nbr_out - skip) as u64;
            self.input.drain(..nbr_in * ch);
        }

        out
    }

    /// End the stream: zero-pad the buffered partial chunk (and, if needed,
    /// further silence to drain the filter) and return the remaining output,
    /// trimmed so that the total output of the stream is exactly
    /// `round(input_frames × ratio)` frames. The resampler is then reset and
    /// ready for a new stream.
    pub fn flush(&mut self) -> Vec<f32> {
        let ch = self.channels;
        let expected = (self.frames_in as f64 * self.ratio).round() as u64;
        let mut out = Vec::new();
        let mut produced = self.frames_out;
        // Each pass feeds at least one chunk of (padded) input, so the filter
        // latency is drained after a few passes at most.
        let mut passes = 0;
        while produced < expected && passes < 64 {
            self.input.resize(self.chunk * ch, 0.0);
            out.extend(self.drain_chunks());
            produced = self.frames_out;
            passes += 1;
        }
        if produced > expected {
            let excess = ((produced - expected) as usize).min(out.len() / ch);
            out.truncate(out.len() - excess * ch);
        }
        self.resampler.reset();
        self.input.clear();
        self.delay_left = self.resampler.output_delay();
        self.frames_in = 0;
        self.frames_out = 0;
        out
    }
}

/// Map a quality level to rubato sinc interpolation parameters.
fn sinc_params(quality: ResamplerQuality) -> SincInterpolationParameters {
    let (sinc_len, oversampling_factor, interpolation, window) = match quality {
        ResamplerQuality::SincBest => (
            256,
            256,
            SincInterpolationType::Cubic,
            WindowFunction::BlackmanHarris2,
        ),
        ResamplerQuality::SincFast => (
            64,
            128,
            SincInterpolationType::Linear,
            WindowFunction::Hann2,
        ),
        // SincMedium (the default) and the `Linear` fallthrough (which does not
        // call this) use balanced parameters.
        _ => (
            128,
            256,
            SincInterpolationType::Quadratic,
            WindowFunction::Blackman2,
        ),
    };
    SincInterpolationParameters {
        sinc_len,
        f_cutoff: Some(calculate_cutoff(sinc_len, window)),
        interpolation,
        oversampling_factor,
        window,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames_out(rs: &mut StreamResampler, input_frames: usize, channels: usize) -> usize {
        let input = vec![0.1f32; input_frames * channels];
        let out = rs.process(&input);
        assert_eq!(out.len() % channels, 0, "output not frame-aligned");
        out.len() / channels
    }

    #[test]
    fn all_qualities_construct() {
        for q in [
            ResamplerQuality::SincBest,
            ResamplerQuality::SincMedium,
            ResamplerQuality::SincFast,
            ResamplerQuality::Linear,
        ] {
            assert!(
                StreamResampler::new(44100, 48000, 2, q).is_some(),
                "failed to construct resampler for {q:?}"
            );
        }
    }

    #[test]
    fn downsample_produces_roughly_half() {
        let mut rs = StreamResampler::new(96000, 48000, 2, ResamplerQuality::SincMedium).unwrap();
        let frames_in = CHUNK_FRAMES * 50;
        let got = frames_out(&mut rs, frames_in, 2);
        let expected = frames_in / 2;
        let tol = CHUNK_FRAMES * 2;
        assert!(
            got.abs_diff(expected) < tol,
            "downsample frames_out={got}, expected≈{expected}"
        );
    }

    #[test]
    fn upsample_produces_more_frames() {
        let mut rs = StreamResampler::new(44100, 48000, 2, ResamplerQuality::SincMedium).unwrap();
        let frames_in = CHUNK_FRAMES * 50;
        let got = frames_out(&mut rs, frames_in, 2);
        let expected = frames_in * 48000 / 44100;
        let tol = CHUNK_FRAMES * 2;
        assert!(
            got.abs_diff(expected) < tol,
            "upsample frames_out={got}, expected≈{expected}"
        );
    }

    #[test]
    fn mono_is_frame_aligned() {
        let mut rs = StreamResampler::new(88200, 48000, 1, ResamplerQuality::SincFast).unwrap();
        let got = frames_out(&mut rs, CHUNK_FRAMES * 10, 1);
        assert!(got > 0);
    }

    #[test]
    fn flush_makes_total_output_match_input_duration() {
        for (src, dst, ch) in [
            (44100u32, 48000u32, 2usize),
            (96000, 48000, 2),
            (44100, 48000, 1),
        ] {
            let mut rs = StreamResampler::new(src, dst, ch, ResamplerQuality::SincMedium).unwrap();
            // Deliberately not a multiple of the chunk size, fed in odd blocks.
            let total_in = 44_123usize;
            let mut total_out = 0usize;
            let block = vec![0.2f32; 777 * ch];
            let mut fed = 0;
            while fed < total_in {
                let n = 777.min(total_in - fed);
                total_out += rs.process(&block[..n * ch]).len() / ch;
                fed += n;
            }
            let tail = rs.flush();
            assert_eq!(tail.len() % ch, 0);
            total_out += tail.len() / ch;
            let expected = (total_in as f64 * f64::from(dst) / f64::from(src)).round() as usize;
            assert!(
                total_out.abs_diff(expected) <= 1,
                "{src}->{dst} x{ch}: got {total_out}, expected {expected}"
            );
        }
    }

    #[test]
    fn flush_of_empty_stream_is_empty_and_resampler_is_reusable() {
        let mut rs = StreamResampler::new(44100, 48000, 2, ResamplerQuality::SincMedium).unwrap();
        assert!(rs.flush().is_empty());
        let mut total = rs.process(&vec![0.1f32; 5000 * 2]).len() / 2;
        total += rs.flush().len() / 2;
        let expected = (5000.0f64 * 48000.0 / 44100.0).round() as usize;
        assert!(
            total.abs_diff(expected) <= 1,
            "got {total}, expected {expected}"
        );
    }

    #[test]
    fn output_is_time_aligned_with_input() {
        // A step at input frame 4000 must appear near output frame 4000*ratio.
        let mut rs = StreamResampler::new(48000, 96000, 1, ResamplerQuality::SincMedium).unwrap();
        let mut input = vec![0.0f32; 4000];
        input.extend(vec![1.0f32; 4000]);
        let mut out = rs.process(&input);
        out.extend(rs.flush());
        let idx = out.iter().position(|&s| s > 0.5).expect("step present");
        assert!(idx.abs_diff(8000) < 64, "step at {idx}, expected ~8000");
    }
}
