/// Reference audio patterns for decoder validation
///
/// Provides mathematically verifiable audio patterns:
/// - Pure sine waves at known frequencies
/// - Impulse responses for sample-accurate testing
/// - Silence for detecting artifacts
use std::f32::consts::PI;

/// Audio pattern type
#[derive(Debug, Clone, Copy)]
pub enum AudioPattern {
    /// Pure sine wave at specified frequency (Hz)
    SineWave(f32),
    /// Single impulse at start (value 1.0, then silence)
    Impulse,
    /// Complete silence (all zeros)
    Silence,
    /// Stereo sine wave with different frequencies per channel
    StereoSine(f32, f32),
}

/// Generate reference audio pattern
///
/// # Arguments
/// * `pattern` - The audio pattern to generate
/// * `sample_rate` - Sample rate in Hz (e.g., 44100)
/// * `channels` - Number of channels (1 = mono, 2 = stereo)
/// * `duration_secs` - Duration in seconds
///
/// # Returns
/// Interleaved f32 samples in range [-1.0, 1.0]
pub fn generate_pattern(
    pattern: AudioPattern,
    sample_rate: u32,
    channels: u8,
    duration_secs: f32,
) -> Vec<f32> {
    let num_frames = (sample_rate as f32 * duration_secs) as usize;
    let num_samples = num_frames * channels as usize;
    let mut samples = vec![0.0f32; num_samples];

    match pattern {
        AudioPattern::SineWave(frequency) => {
            for frame in 0..num_frames {
                let t = frame as f32 / sample_rate as f32;
                let value = (2.0 * PI * frequency * t).sin();

                for ch in 0..channels as usize {
                    samples[frame * channels as usize + ch] = value;
                }
            }
        }
        AudioPattern::Impulse => {
            // First sample is 1.0, rest are 0.0
            for sample in samples.iter_mut().take(channels as usize) {
                *sample = 1.0;
            }
        }
        AudioPattern::Silence => {
            // Already initialized to zeros
        }
        AudioPattern::StereoSine(freq_left, freq_right) => {
            assert_eq!(channels, 2, "StereoSine requires 2 channels");
            for frame in 0..num_frames {
                let t = frame as f32 / sample_rate as f32;
                let left = (2.0 * PI * freq_left * t).sin();
                let right = (2.0 * PI * freq_right * t).sin();

                samples[frame * 2] = left;
                samples[frame * 2 + 1] = right;
            }
        }
    }

    samples
}

/// Verify that samples match a sine wave pattern within tolerance
///
/// Returns true if samples represent a sine wave at the expected frequency
pub fn verify_sine_wave(
    samples: &[f32],
    sample_rate: u32,
    channels: u8,
    expected_frequency: f32,
    tolerance: f32,
) -> bool {
    let num_frames = samples.len() / channels as usize;

    // Sample at several points and verify they match the expected sine wave
    let test_points = 10;
    let mut matches = 0;

    for i in 0..test_points {
        let frame = (i * num_frames) / test_points;
        if frame >= num_frames {
            continue;
        }

        let t = frame as f32 / sample_rate as f32;
        let expected = (2.0 * PI * expected_frequency * t).sin();

        // Check first channel
        let actual = samples[frame * channels as usize];
        let diff = (actual - expected).abs();

        if diff <= tolerance {
            matches += 1;
        }
    }

    // Require at least 80% of test points to match
    matches >= (test_points * 8 / 10)
}

/// Calculate RMS (Root Mean Square) of samples
///
/// Useful for verifying signal strength and detecting silence
pub fn calculate_rms(samples: &[f32]) -> f32 {
    let sum: f32 = samples.iter().map(|&s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Verify samples are mostly silent (RMS below threshold)
pub fn verify_silence(samples: &[f32], threshold: f32) -> bool {
    calculate_rms(samples) < threshold
}

/// Calculate correlation between two sample buffers
///
/// Returns value in range [-1.0, 1.0] where:
/// - 1.0 = perfectly correlated
/// - 0.0 = uncorrelated
/// - -1.0 = perfectly anti-correlated
pub fn calculate_correlation(samples1: &[f32], samples2: &[f32]) -> f32 {
    assert_eq!(samples1.len(), samples2.len());

    let mean1: f32 = samples1.iter().sum::<f32>() / samples1.len() as f32;
    let mean2: f32 = samples2.iter().sum::<f32>() / samples2.len() as f32;

    let mut numerator = 0.0f32;
    let mut sum1_sq = 0.0f32;
    let mut sum2_sq = 0.0f32;

    for i in 0..samples1.len() {
        let diff1 = samples1[i] - mean1;
        let diff2 = samples2[i] - mean2;
        numerator += diff1 * diff2;
        sum1_sq += diff1 * diff1;
        sum2_sq += diff2 * diff2;
    }

    let denominator = (sum1_sq * sum2_sq).sqrt();
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_sine_wave() {
        let samples = generate_pattern(AudioPattern::SineWave(440.0), 44100, 1, 1.0);
        assert_eq!(samples.len(), 44100);

        // Verify it's actually a sine wave
        assert!(verify_sine_wave(&samples, 44100, 1, 440.0, 0.01));
    }

    #[test]
    fn test_generate_impulse() {
        let samples = generate_pattern(AudioPattern::Impulse, 44100, 2, 0.1);

        // First two samples (stereo) should be 1.0
        assert_eq!(samples[0], 1.0);
        assert_eq!(samples[1], 1.0);

        // Rest should be 0.0
        for &s in &samples[2..100] {
            assert_eq!(s, 0.0);
        }
    }

    #[test]
    fn test_generate_silence() {
        let samples = generate_pattern(AudioPattern::Silence, 44100, 2, 0.5);
        assert!(verify_silence(&samples, 0.001));
    }

    #[test]
    fn test_stereo_sine() {
        let samples = generate_pattern(AudioPattern::StereoSine(440.0, 880.0), 44100, 2, 1.0);
        assert_eq!(samples.len(), 44100 * 2);

        // Extract left and right channels
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let right: Vec<f32> = samples.iter().skip(1).step_by(2).copied().collect();

        // Verify each channel
        assert!(verify_sine_wave(&left, 44100, 1, 440.0, 0.01));
        assert!(verify_sine_wave(&right, 44100, 1, 880.0, 0.01));
    }

    #[test]
    fn test_rms_calculation() {
        // Silence should have RMS near 0
        let silence = vec![0.0f32; 1000];
        assert!(calculate_rms(&silence) < 0.001);

        // Full-scale square wave should have RMS = 1.0
        let square = vec![1.0f32; 1000];
        assert!((calculate_rms(&square) - 1.0).abs() < 0.001);

        // Sine wave RMS should be ~0.707 (1/sqrt(2))
        let sine = generate_pattern(AudioPattern::SineWave(440.0), 44100, 1, 1.0);
        let rms = calculate_rms(&sine);
        assert!((rms - 0.707).abs() < 0.01);
    }

    #[test]
    fn test_correlation() {
        let samples1 = generate_pattern(AudioPattern::SineWave(440.0), 44100, 1, 1.0);
        let samples2 = generate_pattern(AudioPattern::SineWave(440.0), 44100, 1, 1.0);
        let samples3 = generate_pattern(AudioPattern::SineWave(880.0), 44100, 1, 1.0);

        // Same signal should have correlation ~1.0
        let corr_same = calculate_correlation(&samples1, &samples2);
        assert!(corr_same > 0.99);

        // Different frequencies should have lower correlation
        let corr_diff = calculate_correlation(&samples1, &samples3);
        assert!(corr_diff.abs() < 0.5);
    }
}
