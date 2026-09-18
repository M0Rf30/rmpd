# Test Audio Fixtures

Pre-generated minimal audio files for rmpd compatibility tests.

## Files

### Basic Format Tests
- `basic.flac` - FLAC with standard metadata (20KB)
- `basic.mp3` - MP3 with ID3v2 tags (8KB)
- `basic.ogg` - OGG Vorbis with comments (7KB)
- `basic.opus` - Opus at 48kHz (18KB)
- `basic.m4a` - M4A/AAC with iTunes tags (25KB)
- `basic.wav` - WAV PCM (172KB)
- `basic.wv` - WavPack (v1-v5) with APEv2 tags (43KB)

### Special Cases
- `unicode.flac` - Unicode metadata (Japanese, Russian, Greek, Arabic) (20KB)
- `minimal.flac` - Minimal metadata (title, artist, album only) (20KB)
- `extended.flac` - Extended metadata (composer, album artist, disc, track) (20KB)

**Total size: 354KB (362,312 bytes across 10 fixtures)**

## Metadata Reference

### basic.flac
```
Title:  Test Song
Artist: Test Artist
Album:  Test Album
Genre:  Rock
Date:   2024
Track:  1
Duration: 1 second
Sample Rate: 44100 Hz
Channels: 2 (stereo)
```

### basic.wv
```
Title:  Test Song WV
Artist: Test Artist WV
Album:  Test Album WV
Duration: 1 second
Sample Rate: 44100 Hz
Channels: 2 (stereo)
```

### unicode.flac
```
Title:  テストソング (Japanese)
Artist: Тестовый исполнитель (Russian)
Album:  Τεστ Άλμπουμ (Greek)
Genre:  الموسيقى (Arabic)
```

### extended.flac
```
Title:       Extended Metadata
Artist:      Extended Artist
Album:       Extended Album
Album Artist: Various Artists
Composer:    Test Composer
Genre:       Jazz
Date:        2024-03-15
Track:       5
Disc:        2
```

## Generation

`basic.flac`, `basic.mp3`, `basic.ogg`, `basic.opus`, `basic.m4a`, `basic.wav`, `unicode.flac`,
`minimal.flac` and `extended.flac` were generated using FFmpeg with 1 second of 440Hz sine wave:

```bash
./generate_fixtures.sh
```

`basic.wv` is **not** covered by that script; it was generated separately with:

```bash
ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a wavpack \
  -metadata title="Test Song WV" -metadata artist="Test Artist WV" \
  -metadata album="Test Album WV" basic.wv
```

The generation scripts/commands are included for reproducibility but are **not required** for
running tests. All fixtures are committed to the repository; only regenerate if adding new test
scenarios, changing metadata requirements, or updating audio properties.

## Usage in Tests

Tests load fixtures directly from this directory:

```rust
use std::path::PathBuf;

let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures/samples/basic.flac");
```

No FFmpeg installation required for running tests!

