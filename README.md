<p align="center">
  <img src="assets/rmpd-logo.png" alt="rmpd — music player daemon" width="420">
</p>

<p align="center">
  <a href="https://github.com/M0Rf30/rmpd/actions/workflows/ci.yml"><img src="https://github.com/M0Rf30/rmpd/workflows/CI/badge.svg" alt="CI"></a>
  <a href="https://github.com/M0Rf30/rmpd/actions/workflows/security.yml"><img src="https://github.com/M0Rf30/rmpd/workflows/Security%20Audit/badge.svg" alt="Security Audit"></a>
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="License"></a>
</p>

**rmpd** is a modern, high-performance, memory-safe music server written in pure Rust. It aims for 100% compatibility with the Music Player Daemon (MPD) protocol while providing first-class extensibility through a plugin architecture.

## Features

- 🎵 **MPD Protocol Compatible** - Works with existing MPD clients (ncmpcpp, mpc, Cantata)
- 🦀 **Pure Rust** - Memory-safe, fast, and reliable
- 🔌 **Extensible** - Plugin system for decoders, outputs, and inputs
- 🎧 **High-Quality Audio** - DSD support, ReplayGain, gapless playback, crossfade
- 🎼 **Format Support** - FLAC, MP3, Ogg Vorbis, WAV, AAC, DSD (DoP and native)
- 🏠 **Multi-Room Ready** - Snapcast integration for synchronized playback
- 🖥️ **Desktop Integration** - Native MPRIS D-Bus interface (media keys, `playerctl`, GNOME/KDE) plus mDNS auto-discovery
- 📱 **Multi-Protocol** - MPD and OpenSubsonic support (planned)
- ⚡ **Efficient** - Runs on everything from Raspberry Pi to high-end servers

## Architecture

```
rmpd/
├── rmpd/               # Main binary
├── rmpd-core/          # Core types and traits
├── rmpd-protocol/      # MPD protocol implementation
├── rmpd-player/        # Audio playback engine
├── rmpd-library/       # Music library/database
├── rmpd-plugin/        # Plugin system
└── rmpd-stream/        # Streaming support
```

## Quick Start

### Prerequisites

**System dependencies:**

```bash
# Ubuntu/Debian
sudo apt-get install libasound2-dev pkg-config

# macOS
brew install pkg-config
```

### Build

```bash
cargo build --release
```

### Run

```bash
./target/release/rmpd --bind 127.0.0.1 --port 6600 --music-dir ~/Music
```

### Test with mpc

```bash
# Check status
mpc status

# Update library
mpc update

# Add and play music
mpc add /
mpc play
```

## Configuration

Create `~/.config/rmpd/rmpd.toml`:

```toml
[general]
music_directory = "~/Music"
playlist_directory = "~/.config/rmpd/playlists"
db_file = "~/.config/rmpd/database.db"
log_level = "info"

[network]
bind_address = "127.0.0.1"
port = 6600
mpris = true

[audio]
default_output = "alsa"
gapless = true
replay_gain = "auto"
buffer_time = 500

[[output]]
name = "ALSA Output"
type = "alsa"
enabled = true
device = "default"

[[output]]
name = "Snapcast Output"
type = "snapcast"
enabled = false
fifo_path = "/tmp/snapfifo"
```

See [rmpd.toml](rmpd.toml) for a complete configuration example.

## Desktop Integration (MPRIS)

rmpd exposes a native [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/) interface on the session D-Bus as `org.mpris.MediaPlayer2.rmpd`. This lets Linux desktops (GNOME Shell, KDE Plasma), `playerctl`, lock screens, and multimedia keys discover and control rmpd directly — no external bridge such as `mpDris2` required.

It is enabled by default and can be toggled with `mpris` under `[network]`. Verify it with:

```bash
playerctl -p rmpd metadata
busctl --user introspect org.mpris.MediaPlayer2.rmpd /org/mpris/MediaPlayer2
```

rmpd also advertises itself over **mDNS/Zeroconf** so MPD clients on the local network can auto-discover the server.

## Audio Format Support

### Supported Formats

- **Lossless**: FLAC, WAV, ALAC, APE, WavPack, TrueAudio
- **Lossy**: MP3, Ogg Vorbis, Opus, AAC, MP4
- **High-Resolution**: DSD (DSF, DFF) with DoP and native playback
- **Streaming**: HTTP streams, Icecast, internet radio

### DSD Support

rmpd includes comprehensive DSD support:
- DSD64, DSD128, DSD256 and higher sample rates
- DoP (DSD over PCM) for wider DAC compatibility
- Native DSD playback for compatible hardware
- Automatic format detection and conversion

## Development

### Running Tests

```bash
cargo test --workspace --all-features
```

### Linting

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --workspace --all-targets --all-features
```

### CI/CD

This project uses GitHub Actions for CI/CD with:
- Multi-platform testing (Ubuntu, macOS)
- Multi-architecture builds (x86_64, ARM64)
- Strict linting with Clippy
- Security audits with cargo-audit and cargo-deny
- Code coverage reporting
- Automated dependency updates via Renovate

See [CI.md](CI.md) for detailed CI/CD documentation.

## Current Status

### Implemented ✅

- **Core Infrastructure**
  - MPD protocol server (TCP/Unix sockets)
  - Event bus system
  - Configuration management
  - Logging with tracing

- **Audio Playback**
  - Multi-format decoding (FLAC, MP3, Vorbis, WAV, AAC, DSD)
  - High-rate DSD support (DSD128, DSD256+)
  - Multiple output types (ALSA, PulseAudio, PipeWire)
  - Gapless playback
  - ReplayGain support

- **Library Management**
  - Filesystem scanning
  - SQLite database
  - Metadata extraction with lofty
  - Full-text search with tantivy
  - Album art support

- **MPD Protocol**
  - Core playback commands (play, pause, stop, seek)
  - Queue management (add, delete, move, shuffle)
  - Database queries (find, search, list)
  - Status and statistics
  - Playlist management
  - Output control

- **Desktop Integration**
  - Native MPRIS D-Bus interface (`org.mpris.MediaPlayer2.rmpd`)
  - Media keys, `playerctl`, and GNOME/KDE media controls
  - mDNS/Zeroconf service advertisement for client auto-discovery

### In Progress 🚧

- Advanced playback features (crossfade, MixRamp)
- Plugin system
- Snapcast integration
- Complete MPD protocol coverage
- OpenSubsonic support

## Compatibility

### Tested MPD Clients

- ✅ **mpc** - Command-line client
- ✅ **ncmpcpp** - TUI client
- ✅ **Cantata** - Qt GUI client
- ✅ **rmpc** - Modern TUI client
- 🚧 **MPDroid** - Android client (testing in progress)
- 🚧 **MPDluxe** - iOS client (testing in progress)

### Multi-Room Audio

- 🚧 **Snapcast** - Synchronous multi-room playback (integration in progress)

## Performance

rmpd is designed for efficiency:
- **Startup time**: < 500ms with 100k song library
- **Memory usage**: < 20MB idle, < 150MB with 100k songs loaded
- **CPU usage**: < 5% during FLAC playback
- **MSRV**: Rust 1.75.0+

## Project Goals

1. **100% MPD Compatibility** - Drop-in replacement for MPD
2. **Modern Architecture** - Clean, modular, testable code
3. **Extensibility** - Plugin system for community contributions
4. **Performance** - Efficient resource usage
5. **Multi-Protocol** - MPD, OpenSubsonic support

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Make your changes with tests
4. Run `cargo fmt` and `cargo clippy`
5. Submit a pull request

See [CI.md](CI.md) for development guidelines and CI/CD information.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Acknowledgments

- Inspired by the original [Music Player Daemon](https://www.musicpd.org/)
- Built with modern Rust audio libraries: [Symphonia](https://github.com/pdeljanov/Symphonia), [cpal](https://github.com/RustAudio/cpal), [lofty](https://github.com/Serial-ATA/lofty-rs)
- Special thanks to the Rust audio community

## Links

- **Documentation**: [CI.md](CI.md) - CI/CD and development guide
- **MPD Protocol**: [MPD Protocol Documentation](https://mpd.readthedocs.io/en/latest/protocol.html)
- **Issue Tracker**: [GitHub Issues](https://github.com/M0Rf30/rmpd/issues)
- **Discussions**: [GitHub Discussions](https://github.com/M0Rf30/rmpd/discussions)
