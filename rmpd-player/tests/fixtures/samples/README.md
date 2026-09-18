# Decoder Test Fixtures

Pre-generated audio files for decoder validation tests.

## Files

### Primary Test Files (1kHz sine wave, 1 second)
- **sine_1khz.flac** (74KB) - FLAC lossless, 44.1kHz stereo, 24-bit
- **sine_1khz.mp3** (14KB) - MP3 VBR quality 2, 44.1kHz stereo
- **sine_1khz.ogg** (8KB) - OGG Vorbis quality 5, 44.1kHz stereo
- **sine_1khz.opus** (23KB) - Opus 128kbps, **48kHz** stereo (Opus requirement)
- **sine_1khz.m4a** (25KB) - AAC 192kbps, 44.1kHz stereo
- **sine_1khz.wav** (172KB) - PCM 16-bit, 44.1kHz stereo

### Seek Test Files
- **sine_440hz.flac** (73KB) - 440Hz sine wave for seek accuracy tests, 44.1kHz stereo, 24-bit

### Special Test Files
- **silence.flac** (8KB) - Silence for artifact detection, 0.5 seconds
- **highres.flac** (139KB) - High-resolution: 96kHz, 24-bit stereo
- **mono.flac** (98KB) - Mono audio test, 440Hz, 44.1kHz, 24-bit

## Total Size
635KB (650,024 bytes across 10 fixtures) - small enough to commit to repository

## Purpose

These fixtures enable decoder tests to run without requiring FFmpeg installation:
- Format detection and audio property extraction
- Lossless decode accuracy (FLAC, WAV)
- Lossy decode quality verification (MP3, OGG, Opus, M4A)
- Seek accuracy testing
- Edge case handling (silence, high-res, mono)

## Regeneration

To regenerate fixtures (requires FFmpeg):

```bash
cd rmpd-player/tests/fixtures/samples
./generate_fixtures.sh
```

## Test Patterns

All audio files contain mathematically verifiable patterns:
- **Sine waves**: Pure tone at known frequency (440Hz or 1000Hz)
- **Silence**: All zeros for artifact detection

These patterns allow verification that decoders produce correct output:
- Lossless formats (FLAC, WAV) should match reference pattern exactly
- Lossy formats (MP3, OGG, etc.) should resemble pattern within tolerance

## FFmpeg Commands

Examples of commands used (see `generate_fixtures.sh` for the complete list). Every fixture is
generated with `-af "volume=9"` to boost FFmpeg's low-amplitude sine generator (~0.088) to a
usable test signal (~0.8); omitting it produces a file that does not match what's committed:

```bash
# FLAC (lossless)
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 \
  sine_1khz.flac

# MP3 (VBR)
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 44100 -ac 2 \
  -codec:a libmp3lame -q:a 2 sine_1khz.mp3

# Opus (48kHz required)
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 48000 -ac 2 \
  -codec:a libopus -b:a 128k sine_1khz.opus

# High-resolution FLAC (96kHz, 24-bit)
ffmpeg -y -f lavfi -i "sine=frequency=1000:duration=1" -af "volume=9" -ar 96000 -ac 2 \
  -sample_fmt s32 highres.flac
```

Committed FLAC fixtures currently probe as 24-bit even where the script does not pass
`-sample_fmt` explicitly (`sine_1khz.flac`, `sine_440hz.flac`, `mono.flac`) - this follows
whatever bit depth the installed FFmpeg's FLAC encoder defaults to, so re-running the script
with a different FFmpeg version may not byte-for-byte reproduce the committed files even though
the audio content matches.
