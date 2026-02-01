/// Test fixtures for decoder validation
///
/// This module provides audio test fixtures for validating decoder behavior:
/// - **Reference patterns**: Mathematically verifiable sine waves, impulses
/// - **Format-specific files**: FLAC, MP3, OGG, Opus, M4A, WAV, DSD
/// - **Pre-generated samples**: Small files for CI (no FFmpeg required)
/// - **Generator**: FFmpeg-based generator for advanced testing (optional)

pub mod generator;
pub mod pregenerated;
pub mod reference;
