# Test Audio Fixtures

Pre-generated minimal audio files for rmpd compatibility tests.

## Files

### Basic Format Tests
- `basic.flac` - FLAC with standard metadata (21KB)
- `basic.mp3` - MP3 with ID3v2 tags (8KB)
- `basic.ogg` - OGG Vorbis with comments (7KB)
- `basic.opus` - Opus at 48kHz (18KB)
- `basic.m4a` - M4A/AAC with iTunes tags (25KB)
- `basic.wav` - WAV PCM (173KB)

### Special Cases
- `unicode.flac` - Unicode metadata (Japanese, Russian, Greek, Arabic) (21KB)
- `minimal.flac` - Minimal metadata (title, artist, album only) (21KB)
- `extended.flac` - Extended metadata (composer, album artist, disc, track) (21KB)

**Total size: 344KB**

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

These files were generated using FFmpeg with 1 second of 440Hz sine wave:

```bash
./generate_fixtures.sh
```

The generation script is included for reproducibility but is **not required** for running tests. All fixtures are committed to the repository.

## Usage in Tests

Tests load fixtures directly from this directory:

```rust
use std::path::PathBuf;

let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures/samples/basic.flac");
```

No FFmpeg installation required for running tests!

## Regeneration

To regenerate fixtures (requires FFmpeg):
```bash
cd tests/fixtures/samples
./generate_fixtures.sh
```

Only needed if:
- Adding new test scenarios
- Changing metadata requirements
- Updating audio properties
