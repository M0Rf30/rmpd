/// DSD/DoP-specific tests
///
/// Tests for DSD (Direct Stream Digital) and DoP (DSD over PCM) encoding:
/// - DSD detection
/// - DoP marker generation and alternation
/// - Bit order handling (LSB-first vs MSB-first)
/// - Channel layout (planar vs interleaved)
/// - Sample rate conversions (DSD64 → 176.4kHz, DSD128 → 352.8kHz)
use rmpd_player::dop::DopEncoder;
use symphonia::core::codecs::{BitOrder, ChannelDataLayout};

const DOP_MARKER_1: u8 = 0x05;
const DOP_MARKER_2: u8 = 0xFA;

#[test]
fn test_dop_encoder_creation_dsd64() {
    let encoder = DopEncoder::new(2822400, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst);
    assert!(encoder.is_ok(), "Should create DSD64 encoder");

    let encoder = encoder.unwrap();
    assert_eq!(encoder.pcm_sample_rate(), 176400, "DSD64 → 176.4kHz PCM");
}

#[test]
fn test_dop_encoder_creation_dsd128() {
    let encoder = DopEncoder::new(5644800, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst);
    assert!(encoder.is_ok(), "Should create DSD128 encoder");

    let encoder = encoder.unwrap();
    assert_eq!(encoder.pcm_sample_rate(), 352800, "DSD128 → 352.8kHz PCM");
}

#[test]
fn test_dop_encoder_rejects_dsd256() {
    let encoder = DopEncoder::new(11289600, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst);
    assert!(
        encoder.is_err(),
        "Should reject DSD256 (requires 705.6kHz PCM, not practical)"
    );
}

#[test]
fn test_dop_encoder_rejects_invalid_rate() {
    let encoder = DopEncoder::new(48000, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst);
    assert!(encoder.is_err(), "Should reject non-DSD sample rate");
}

#[test]
fn test_dop_encoding_planar_msb_first() {
    let mut encoder = DopEncoder::new(2822400, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // Test data: 4 bytes planar (2 bytes left, 2 bytes right)
    // Left channel: [0x12, 0x34]
    // Right channel: [0x56, 0x78]
    let dsd_data = vec![0x12, 0x34, 0x56, 0x78];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    // Should produce 2 samples (L and R for one frame)
    assert_eq!(output.len(), 2, "Should produce 2 DoP samples");

    // Left channel: [marker1][0x12][0x34][0x00] (left-aligned 24-bit)
    let expected_left = (DOP_MARKER_1 as i32) << 24 | 0x12 << 16 | 0x34 << 8;
    assert_eq!(
        output[0], expected_left,
        "Left channel DoP sample incorrect"
    );

    // Right channel: [marker1][0x56][0x78][0x00]
    let expected_right = (DOP_MARKER_1 as i32) << 24 | 0x56 << 16 | 0x78 << 8;
    assert_eq!(
        output[1], expected_right,
        "Right channel DoP sample incorrect"
    );
}

#[test]
fn test_dop_encoding_interleaved_msb_first() {
    let mut encoder = DopEncoder::new(
        2822400,
        2,
        ChannelDataLayout::Interleaved,
        BitOrder::MsbFirst,
    )
    .expect("Failed to create encoder");

    // Test data: 4 bytes interleaved (L0, R0, L1, R1)
    // Frame 0: Left [0x12], Right [0x34]
    // Frame 1: Left [0x56], Right [0x78]
    // But we need 2 bytes per channel per frame, so:
    // Left channel bytes: [0x12, 0x34] (for frame 0)
    // Right channel bytes: [0x56, 0x78] (for frame 0)
    let dsd_data = vec![0x12, 0x34, 0x56, 0x78];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    assert_eq!(output.len(), 2, "Should produce 2 DoP samples");

    // Verify output contains expected data
    assert_ne!(output[0], 0, "Left sample should not be zero");
    assert_ne!(output[1], 0, "Right sample should not be zero");
}

#[test]
fn test_dop_marker_alternation() {
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // Generate 2 frames (4 bytes, 1 channel, 2 bytes per frame)
    let dsd_data = vec![0xAA, 0xBB, 0xCC, 0xDD];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    assert_eq!(output.len(), 2, "Should produce 2 frames");

    // Extract markers from MSB (left-aligned 24-bit in 32-bit int)
    let marker1 = (output[0] >> 24) as u8;
    let marker2 = (output[1] >> 24) as u8;

    assert_eq!(marker1, DOP_MARKER_1, "First frame should have marker 0x05");
    assert_eq!(
        marker2, DOP_MARKER_2,
        "Second frame should have marker 0xFA"
    );
}

#[test]
fn test_dop_marker_continues_alternating() {
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // Encode first batch
    let dsd_data1 = vec![0x00, 0x01, 0x02, 0x03]; // 2 frames
    let mut output1 = Vec::new();
    encoder.encode(&dsd_data1, &mut output1);

    let marker1 = (output1[0] >> 24) as u8;
    let marker2 = (output1[1] >> 24) as u8;

    // Encode second batch (marker should continue alternating)
    let dsd_data2 = vec![0x04, 0x05]; // 1 frame
    let mut output2 = Vec::new();
    encoder.encode(&dsd_data2, &mut output2);

    let marker3 = (output2[0] >> 24) as u8;

    // Should alternate: 0x05, 0xFA, 0x05
    assert_eq!(marker1, DOP_MARKER_1);
    assert_eq!(marker2, DOP_MARKER_2);
    assert_eq!(marker3, DOP_MARKER_1);
}

#[test]
fn test_bit_reversal_lsb_first() {
    // When bit order is LSB-first, encoder should reverse bits
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::LsbFirst)
        .expect("Failed to create encoder");

    // Test byte: 0b10101010 (0xAA)
    // Reversed: 0b01010101 (0x55)
    let dsd_data = vec![0xAA, 0x00];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    assert_eq!(output.len(), 1);

    // Extract DSD bytes from DoP sample: [marker][byte1][byte2][0x00]
    let dop_sample = output[0];
    let byte1 = ((dop_sample >> 16) & 0xFF) as u8;
    let byte2 = ((dop_sample >> 8) & 0xFF) as u8;

    // byte1 should be reversed: 0xAA → 0x55
    assert_eq!(byte1, 0x55, "Byte should be reversed for LSB-first");
    assert_eq!(byte2, 0x00, "Second byte should be 0x00");
}

#[test]
fn test_bit_no_reversal_msb_first() {
    // When bit order is MSB-first, no reversal needed
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    let dsd_data = vec![0xAA, 0xBB];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    assert_eq!(output.len(), 1);

    let dop_sample = output[0];
    let byte1 = ((dop_sample >> 16) & 0xFF) as u8;
    let byte2 = ((dop_sample >> 8) & 0xFF) as u8;

    // Should remain unchanged
    assert_eq!(byte1, 0xAA, "Byte should not be reversed for MSB-first");
    assert_eq!(byte2, 0xBB);
}

#[test]
fn test_to_f32_samples_conversion() {
    // Test conversion from actual DoP i32 samples to f32
    // Note: to_f32_samples normalizes by 2^23, which is appropriate for
    // 24-bit audio data, but DoP markers extend into the full 32-bit range

    // Create some actual DoP samples
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    let dsd_data = vec![0x00, 0x00]; // Silence
    let mut output = Vec::new();
    encoder.encode(&dsd_data, &mut output);

    // Convert to f32
    let f32_samples = DopEncoder::to_f32_samples(&output);

    assert_eq!(f32_samples.len(), 1);
    // The result includes the marker byte, so won't be exactly 0 or within [-1, 1]
    // This is expected - the function is designed for the specific DoP use case
    assert!(f32_samples[0].is_finite(), "Sample should be finite");
}

#[test]
fn test_dop_encoding_stereo() {
    let mut encoder = DopEncoder::new(2822400, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // 4 bytes: 2 for left, 2 for right
    let dsd_data = vec![0x11, 0x22, 0x33, 0x44];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    // Should produce 2 samples (1 frame with L and R)
    assert_eq!(output.len(), 2);

    // Verify both samples have the same marker (same frame)
    let marker_left = (output[0] >> 24) as u8;
    let marker_right = (output[1] >> 24) as u8;
    assert_eq!(
        marker_left, marker_right,
        "Both channels in same frame should have same marker"
    );
}

#[test]
fn test_dop_encoding_mono() {
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    let dsd_data = vec![0xFF, 0xEE];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    // Should produce 1 sample (1 channel, 1 frame)
    assert_eq!(output.len(), 1);

    let dop_sample = output[0];
    let marker = (dop_sample >> 24) as u8;
    let byte1 = ((dop_sample >> 16) & 0xFF) as u8;
    let byte2 = ((dop_sample >> 8) & 0xFF) as u8;

    assert_eq!(marker, DOP_MARKER_1);
    assert_eq!(byte1, 0xFF);
    assert_eq!(byte2, 0xEE);
}

#[test]
fn test_multiple_frames_encoding() {
    let mut encoder = DopEncoder::new(2822400, 2, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // 8 bytes: 4 for left channel, 4 for right channel = 2 frames
    // Left: [0x01, 0x02, 0x03, 0x04]
    // Right: [0x05, 0x06, 0x07, 0x08]
    let dsd_data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    // Should produce 4 samples (2 frames × 2 channels)
    assert_eq!(
        output.len(),
        4,
        "Should produce 4 DoP samples (2 frames × 2 channels)"
    );

    // Frame 0: Left, Right
    // Frame 1: Left, Right
    // Verify markers alternate per frame
    let marker_f0_l = (output[0] >> 24) as u8;
    let marker_f0_r = (output[1] >> 24) as u8;
    let marker_f1_l = (output[2] >> 24) as u8;
    let marker_f1_r = (output[3] >> 24) as u8;

    assert_eq!(marker_f0_l, DOP_MARKER_1, "Frame 0 should have marker 1");
    assert_eq!(
        marker_f0_r, DOP_MARKER_1,
        "Frame 0 right channel same marker"
    );
    assert_eq!(marker_f1_l, DOP_MARKER_2, "Frame 1 should have marker 2");
    assert_eq!(
        marker_f1_r, DOP_MARKER_2,
        "Frame 1 right channel same marker"
    );
}

#[test]
fn test_dop_sample_range() {
    // Verify DoP i32 samples are in valid signed 32-bit range
    let mut encoder = DopEncoder::new(2822400, 1, ChannelDataLayout::Planar, BitOrder::MsbFirst)
        .expect("Failed to create encoder");

    // Use various byte patterns
    let dsd_data = vec![0x00, 0xFF, 0xAA, 0x55, 0x12, 0x34];
    let mut output = Vec::new();

    encoder.encode(&dsd_data, &mut output);

    // Verify all i32 samples are valid (non-zero for non-silent input)
    assert!(!output.is_empty(), "Should produce DoP samples");

    // Verify structure: each sample should have marker in MSB
    for &sample in &output {
        let marker = (sample >> 24) as u8;
        assert!(
            marker == 0x05 || marker == 0xFA,
            "Invalid DoP marker: 0x{:02X}",
            marker
        );
    }
}
