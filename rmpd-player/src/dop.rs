/// DoP (DSD over PCM) encoder
///
/// Packs DSD data into 24-bit PCM frames for transmission over standard PCM audio interfaces.
/// The DAC recognizes the DoP markers and extracts the original DSD stream.
///
/// DoP Format:
/// - DSD64 (2.8224 MHz) → 176.4 kHz PCM (2.8224 / 16 = 176.4)
/// - DSD128 (5.6448 MHz) → 352.8 kHz PCM (5.6448 / 16 = 352.8)
///
/// Each PCM sample contains:
/// - Byte 0: Marker (0x05 or 0xFA alternating)
/// - Byte 1: 8 bits of DSD data
/// - Byte 2: 8 more bits of DSD data
///
/// Reference: https://dsd-guide.com/sites/default/files/white-papers/DoP_openStandard_1v1.pdf
use rmpd_core::error::{Result, RmpdError};
use symphonia::core::codecs::{BitOrder, ChannelDataLayout};

const DOP_MARKER_1: u8 = 0x05;
const DOP_MARKER_2: u8 = 0xFA;

/// Reverse the bits in a byte
/// Used when source DSD is LSB-first but DAC expects MSB-first in DoP
#[inline]
fn reverse_bits(byte: u8) -> u8 {
    let mut result = 0u8;
    let mut b = byte;
    for _ in 0..8 {
        result = (result << 1) | (b & 1);
        b >>= 1;
    }
    result
}

/// DoP encoder that converts DSD data to DoP-encoded PCM
pub struct DopEncoder {
    dsd_sample_rate: u32,
    channels: usize,
    marker_toggle: bool,
    channel_layout: ChannelDataLayout,
    bit_order: BitOrder,
}

impl DopEncoder {
    pub fn new(
        dsd_sample_rate: u32,
        channels: usize,
        channel_layout: ChannelDataLayout,
        bit_order: BitOrder,
    ) -> Result<Self> {
        // Validate DSD sample rate
        match dsd_sample_rate {
            2822400 => {} // DSD64
            5644800 => {} // DSD128
            11289600 => {
                return Err(RmpdError::Player(
                    "DSD256 not supported via DoP (would require 705.6kHz PCM)".to_owned(),
                ))
            }
            _ => {
                return Err(RmpdError::Player(format!(
                    "Unsupported DSD sample rate: {dsd_sample_rate}"
                )))
            }
        }

        Ok(Self {
            dsd_sample_rate,
            channels,
            marker_toggle: false,
            channel_layout,
            bit_order,
        })
    }

    /// Get the PCM sample rate for DoP encoding
    pub fn pcm_sample_rate(&self) -> u32 {
        // Each PCM frame contains 16 DSD bits (2 bytes)
        self.dsd_sample_rate / 16
    }

    /// Convert DSD samples to DoP-encoded 24-bit PCM samples
    ///
    /// Input: Raw DSD data (1 bit per sample, packed into bytes)
    /// Output: 24-bit PCM samples (as i32) ready for playback
    ///
    /// Handles both planar and interleaved channel layouts
    /// Handles both LSB-first and MSB-first bit orders
    ///
    /// DoP PCM output (always interleaved):
    /// [L_marker_DSD0-15, R_marker_DSD0-15, ...] for each frame
    pub fn encode(&mut self, dsd_data: &[u8], output: &mut Vec<i32>) {
        // Each DoP PCM sample contains 16 DSD bits (2 bytes per channel)
        let dsd_bytes_per_channel_per_frame = 2;

        match self.channel_layout {
            ChannelDataLayout::Planar => {
                // Planar layout: all bytes for channel 0, then all for channel 1
                let bytes_per_channel = dsd_data.len() / self.channels;
                let num_frames = bytes_per_channel / dsd_bytes_per_channel_per_frame;

                // Reserve space for output (one sample per channel per frame, interleaved)
                output.clear();
                output.reserve(num_frames * self.channels);

                for frame_idx in 0..num_frames {
                    // Alternate marker for each frame
                    let marker = if self.marker_toggle {
                        DOP_MARKER_2
                    } else {
                        DOP_MARKER_1
                    };
                    self.marker_toggle = !self.marker_toggle;

                    // Process each channel (convert planar to interleaved output)
                    for ch in 0..self.channels {
                        // Calculate offset in planar layout
                        let channel_offset = ch * bytes_per_channel;
                        let dsd_offset =
                            channel_offset + (frame_idx * dsd_bytes_per_channel_per_frame);

                        // Get 2 bytes of DSD data for this channel
                        let dsd_byte1 = dsd_data[dsd_offset];
                        let dsd_byte2 = dsd_data[dsd_offset + 1];

                        // Apply bit reversal (LSB-first -> MSB-first)
                        let (byte1, byte2) = if self.bit_order == BitOrder::LsbFirst {
                            (reverse_bits(dsd_byte1), reverse_bits(dsd_byte2))
                        } else {
                            (dsd_byte1, dsd_byte2)
                        };

                        // Pack into 24-bit DoP sample: [marker, byte1, byte2]
                        // For 32-bit output (S32_LE), left-align by shifting left 8 bits
                        // Result: [marker][byte1][byte2][0x00]
                        let dop_sample = ((marker as i32) << 24)
                            | ((byte1 as i32) << 16)
                            | ((byte2 as i32) << 8);

                        output.push(dop_sample);
                    }
                }
            }
            ChannelDataLayout::Interleaved => {
                // Interleaved layout: [L0, R0, L1, R1, L2, R2, ...]
                let num_frames = dsd_data.len() / (self.channels * dsd_bytes_per_channel_per_frame);

                // Reserve space for output
                output.clear();
                output.reserve(num_frames * self.channels);

                for frame_idx in 0..num_frames {
                    // Alternate marker for each frame
                    let marker = if self.marker_toggle {
                        DOP_MARKER_2
                    } else {
                        DOP_MARKER_1
                    };
                    self.marker_toggle = !self.marker_toggle;

                    // Process each channel
                    for ch in 0..self.channels {
                        // Calculate offset in interleaved layout
                        let dsd_offset =
                            (frame_idx * self.channels + ch) * dsd_bytes_per_channel_per_frame;

                        // Get 2 bytes of DSD data for this channel
                        let dsd_byte1 = dsd_data[dsd_offset];
                        let dsd_byte2 = dsd_data[dsd_offset + 1];

                        // Apply bit reversal (LSB-first -> MSB-first)
                        let (byte1, byte2) = if self.bit_order == BitOrder::LsbFirst {
                            (reverse_bits(dsd_byte1), reverse_bits(dsd_byte2))
                        } else {
                            (dsd_byte1, dsd_byte2)
                        };

                        // Pack into 24-bit DoP sample: [marker, byte1, byte2]
                        // For 32-bit output (S32_LE), left-align by shifting left 8 bits
                        // Result: [marker][byte1][byte2][0x00]
                        let dop_sample = ((marker as i32) << 24)
                            | ((byte1 as i32) << 16)
                            | ((byte2 as i32) << 8);

                        output.push(dop_sample);
                    }
                }
            }
        }
    }

    /// Convert DoP 24-bit samples (i32) to f32 for cpal
    pub fn to_f32_samples(dop_i32: &[i32]) -> Vec<f32> {
        dop_i32
            .iter()
            .map(|&sample| {
                // Normalize 24-bit to f32 range [-1.0, 1.0]
                // 24-bit range: -8388608 to 8388607
                (sample as f32) / 8388608.0
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dop_encoder_dsd64() {
        // Test with MSB-first bit order (no reversal needed)
        let mut encoder =
            DopEncoder::new(2822400, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst).unwrap();
        assert_eq!(encoder.pcm_sample_rate(), 176400);

        // Test data: 4 bytes planar format (all left, then all right)
        // Left channel: [0x12, 0x34]
        // Right channel: [0x56, 0x78]
        let dsd_data = vec![0x12, 0x34, 0x56, 0x78];
        let mut output = Vec::new();

        encoder.encode(&dsd_data, &mut output);

        // Should produce 2 samples: L and R for one frame
        assert_eq!(output.len(), 2);

        // First frame, left channel: [marker1][0x12][0x34][0x00] (left-aligned)
        assert_eq!(
            output[0],
            (DOP_MARKER_1 as i32) << 24 | 0x12 << 16 | 0x34 << 8
        );

        // First frame, right channel: [marker1][0x56][0x78][0x00] (left-aligned)
        assert_eq!(
            output[1],
            (DOP_MARKER_1 as i32) << 24 | 0x56 << 16 | 0x78 << 8
        );
    }

    #[test]
    fn test_marker_alternation() {
        // Test with MSB-first bit order
        let mut encoder =
            DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst).unwrap();
        let dsd_data = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let mut output = Vec::new();

        encoder.encode(&dsd_data, &mut output);

        // Extract markers from samples (now in MSB due to left-alignment)
        let marker1 = (output[0] >> 24) as u8;
        let marker2 = (output[1] >> 24) as u8;

        // Markers should alternate
        assert_eq!(marker1, DOP_MARKER_1);
        assert_eq!(marker2, DOP_MARKER_2);
    }
}
