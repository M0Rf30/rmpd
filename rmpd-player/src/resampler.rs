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

        Some(Self {
            resampler,
            channels,
            chunk,
            input: Vec::new(),
            scratch,
        })
    }

    /// Resample one block of interleaved input, returning interleaved output at
    /// the destination rate. Leftover input (less than one chunk) is carried
    /// across calls so block boundaries stay continuous.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.input.extend_from_slice(input);

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
                let mut out_adapter = match InterleavedSlice::new_mut(&mut self.scratch, ch, out_cap)
                {
                    Ok(a) => a,
                    Err(_) => break,
                };
                match self
                    .resampler
                    .process_into_buffer(&in_adapter, &mut out_adapter, Some(&indexing))
                {
                    Ok(counts) => counts,
                    Err(_) => break,
                }
            };

            // Guard against a pathological zero-consumption result that would
            // otherwise spin forever.
            if nbr_in == 0 {
                break;
            }

            out.extend_from_slice(&self.scratch[..nbr_out * ch]);
            self.input.drain(..nbr_in * ch);
        }

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
        ResamplerQuality::SincFast => {
            (64, 128, SincInterpolationType::Linear, WindowFunction::Hann2)
        }
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
        f_cutoff: calculate_cutoff(sinc_len, window),
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
}
