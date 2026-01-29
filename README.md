# rmpd

**rmpd** is a modern, high-performance, memory-safe music server written in pure Rust. It aims for 100% compatibility with the Music Player Daemon (MPD) protocol while providing first-class extensibility through a plugin architecture.

## Features

- 🎵 **MPD Protocol Compatible** - Works with existing MPD clients (ncmpcpp, mpc, Cantata)
- 🦀 **Pure Rust** - Memory-safe, fast, and reliable
- 🔌 **Extensible** - Plugin system for decoders, outputs, and inputs
- 🎧 **High-Quality Audio** - ReplayGain, gapless playback, crossfade
- 🏠 **Multi-Room Ready** - Snapcast integration for synchronized playback
- 📱 **Mobile Friendly** - OpenSubsonic support (planned)
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

### Build

```bash
cargo build --release
```

### Run

```bash
./target/release/rmpd --bind 127.0.0.1 --port 6600
```

### Test with mpc

```bash
mpc status
```

## Configuration

Create `~/.config/rmpd/rmpd.toml`:

```toml
[general]
music_directory = "~/Music"

[network]
bind_address = "127.0.0.1"
port = 6600

[audio]
default_output = "alsa"
gapless = true
```

See [CLAUDE.md](CLAUDE.md) for full configuration options.

## Development Status

### Phase 1: Foundation ✅ **COMPLETED**
- [x] Project structure
- [x] Core types (Song, Queue, PlayerStatus)
- [x] Configuration loading
- [x] MPD protocol parser
- [x] TCP server
- [x] Event bus system
- [x] Response formatting
- [x] Working MPD commands (ping, status, stats, commands)
- [ ] Basic audio playback (Phase 2)
- [ ] SQLite database (Phase 2)

### Phase 2-7: Coming Soon
See [CLAUDE.md](CLAUDE.md) for the full roadmap.

## Project Goals

1. **100% MPD Compatibility** - Drop-in replacement for MPD
2. **Modern Architecture** - Clean, modular, testable code
3. **Extensibility** - Plugin system for community contributions
4. **Performance** - Efficient resource usage
5. **Multi-Protocol** - MPD, RNP, OpenSubsonic support

## Contributing

Contributions are welcome! Please read [CLAUDE.md](CLAUDE.md) for architecture details and development guidelines.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Acknowledgments

- Inspired by the original [Music Player Daemon](https://www.musicpd.org/)
- Built with modern Rust audio libraries: [Symphonia](https://github.com/pdeljanov/Symphonia), [cpal](https://github.com/RustAudio/cpal)
