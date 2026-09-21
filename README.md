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

- 🎵 **MPD Protocol Compatible** — works with existing MPD clients (ncmpcpp, mpc, Cantata, rmpc)
- 🦀 **Pure Rust** — memory-safe; even APE and WavPack decode without C bindings
- 🔌 **Extensible** — compile-time plugin registries for outputs, decoders, and music sources
- 🎧 **High-Quality Audio** — DSD (DoP + PCM fallback), ReplayGain, gapless playback, crossfade
- 🎼 **Format Support** — FLAC, MP3, Ogg Vorbis, WAV, AAC, ALAC, APE, WavPack, DSD and more; see [Format Support](#format-support)
- 🏠 **Multi-Room** — HTTP streaming and Snapcast; see [Integrations](#integrations)
- 🖥️ **Desktop Integration** — MPRIS D-Bus and mDNS auto-discovery; see [Integrations](#integrations)
- 🌐 **Remote Libraries** — OpenSubsonic servers as a music source; see [Integrations](#integrations)

## Architecture

```
rmpd/
├── rmpd/            # CLI entry point / main binary
├── rmpd-core/       # Config, error, event bus, queue, song/tag, state — shared by every crate
├── rmpd-macros/     # #[derive(CommandMetadata)] proc macro for MPD command dispatch
├── rmpd-protocol/   # MPD wire protocol: parser, command dispatch, connection/server, MPRIS
├── rmpd-player/     # Audio engine: symphonia decoding, DSD/DoP, outputs, resampling
├── rmpd-library/    # Filesystem scanner, SQLite database, tag/artwork extraction, search
├── rmpd-plugin/     # Cross-cutting plugin SPI (currently the MusicSource trait)
├── rmpd-source/     # Music-source registry + backends (filesystem, OpenSubsonic)
└── rmpd-stream/     # HTTP(S) streaming input source for internet radio (ICY metadata)
```

## Quick Start

### Prerequisites

Requires Rust 1.85+ (the workspace uses edition 2024).

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

# Play internet radio (any HTTP/HTTPS stream URL)
mpc add https://stream.example/radio.mp3
mpc play
mpc current   # shows the live ICY "now playing" title for streams
```

## Format Support

Library scanning/tagging and playback both go through `symphonia`, but they are not the same list: a file can scan and tag without a decoder to play it.

| Extension(s)                 | Scan / tag / browse | Playback |
| ----------------------------- | :---: | :---: |
| `flac`                        | ✅ | ✅ |
| `mp3`                          | ✅ | ✅ |
| `ogg`, `oga`                   | ✅ | ✅ |
| `opus`                         | ✅ | ❌ — demuxed, no Opus decoder |
| `wav`                          | ✅ | ✅ |
| `aiff`, `aif`                  | ✅ | ✅ |
| `m4a`                          | ✅ | ✅ |
| `aac`                          | ✅ | ✅ |
| `ape` (Monkey's Audio)         | ✅ | ✅ — pure-Rust decoder |
| `wv` (WavPack)                 | ✅ | ✅ — pure-Rust decoder |
| `dsf`, `dff` (DSD)             | ✅ | ✅ — see [DSD](#dsd) |
| `mka`, `webm`                  | ✅ | ✅ |
| `wave`, `mp4`, `alac`, `caf`    | ❌ (not scanned into the library) | ✅ — playable if referenced directly |

Musepack (`.mpc`) is not supported at all: no scan, no tag, no playback.

### DSD

- `.dsf`/`.dff`, DSD64 through DSD256 and higher, all scan, tag, and play.
- DoP (DSD over PCM) sends a native DSD64/DSD128 bitstream to a bit-perfect DAC over a raw ALSA `hw:` device. Opt in with `audio.dop = "yes"` (or `"auto"` to use DoP only when `audio.device` is set) or `RMPD_DOP=1`. DSD256 and higher cannot use DoP (would need 705.6kHz PCM) and always use PCM fallback.
- PCM fallback (the default) decodes DSD to a 44.1kHz-family rate and resamples to the output device's native rate via the configured `resampler_quality`, so a sound server such as PipeWire never resamples internally — avoiding underruns and keeping DSD's ultrasonic noise out of the audible band.

## Configuration

rmpd does not require you to create a config file. On first start, if no
config file is found, rmpd writes a commented starter config to
`~/.config/rmpd/rmpd.toml` and logs the path it chose. You can also manage
this explicitly:

- `--generate-config` writes the starter config on demand and refuses to
  overwrite an existing file.
- `--print-config-path` prints the path that would be used, without writing
  anything.
- `--no-config` skips file discovery entirely and runs on built-in defaults.

### Search order

When `--config` is not given, rmpd searches these locations in order and
uses the first one that exists:

1. the path given to `--config`
2. `$XDG_CONFIG_HOME/rmpd/rmpd.toml` (usually `~/.config/rmpd/rmpd.toml`)
3. `~/.rmpd.toml`
4. `~/.rmpd/rmpd.toml`
5. `/etc/rmpd/rmpd.toml`

This mirrors MPD's own search order (`$XDG_CONFIG_HOME/mpd/mpd.conf`,
`~/.mpdconf`, `~/.mpd/mpd.conf`, `/etc/mpd.conf`).

Every section and key is optional — anything omitted uses a built-in
default, so a partial config is valid. Paths beginning with `~` are
expanded.

A minimal config:

```toml
[general]
music_directory = "~/Music"
log_level = "info"

[network]
bind_address = "127.0.0.1"
port = 6600

[audio]
default_output = "alsa"
replay_gain = "auto"
```

See [rmpd.toml](rmpd.toml) for a complete, annotated configuration example.

Additional `[general]`/`[network]` keys (see [rmpd.toml](rmpd.toml) for full examples):

- `state_file_interval` — seconds between periodic state-file saves; `0` disables periodic saving (default 120)
- `log_file` — write logs to this file instead of stdout (default: stdout)
- `max_playlist_length` — maximum number of songs in the queue (default 16384)
- `save_absolute_paths_in_playlists` — store absolute paths in saved `.m3u` playlists (default false)
- `metadata_to_use` — restrict which tags are read/stored during scans (default: all tags)
- `network.passwords` — array of `password`/`permissions` pairs granting scoped access (default: none)
- `network.default_permissions` / `network.local_permissions` / `network.host_permissions` — permission sets for unauthenticated, local-socket, and per-host clients, mirroring MPD's access control (default: unrestricted)
- `network.max_command_list_size` / `network.max_output_buffer_size` — per-client buffer caps in bytes (defaults 16 MiB / 8 MiB)
- `network.zeroconf_name` — mDNS/Zeroconf service name, `%h` expands to the hostname (default `rmpd@%h`)

### Diagnostics

Unrecognized keys are reported at startup instead of being silently
ignored, with a suggestion when a typo is likely, for example:

```
unknown config key `general.msic_directory` (did you mean `music_directory`?)
```

Keys carried over from `mpd.conf` produce a migration hint pointing at the
rmpd equivalent. Unknown keys only warn — they never stop the daemon.
An explicitly passed `--config` file that cannot be read or parsed is
fatal, since a config you asked for by name should never be silently
substituted with defaults.

A missing or nonexistent `music_directory` is **not** fatal: rmpd warns,
starts anyway, and skips library scanning, matching MPD's
degrade-don't-die behaviour.

`log_level` under `[general]` controls logging (`trace`/`debug`/`info`/
`warn`/`error`). `--verbose` forces `debug`, and the `RUST_LOG` environment
variable overrides both.

The following keys were parsed in older versions but never did anything
and have been removed; a config that still sets them gets a startup
warning naming each one:

- `audio.gapless` — gapless playback is always on
- the entire `[decoder]` section, including `decoder.enabled` — decoders
  come from a compile-time registry
- `database.cache_size`
- `database.fts_enabled`

## Integrations

### MPRIS & mDNS

rmpd exposes a native [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/) interface on the session D-Bus as `org.mpris.MediaPlayer2.rmpd`. This lets Linux desktops (GNOME Shell, KDE Plasma), `playerctl`, lock screens, and multimedia keys discover and control rmpd directly — no external bridge such as `mpDris2` required. It is enabled by default and can be toggled with `mpris` under `[network]`.

```bash
playerctl -p rmpd metadata
busctl --user introspect org.mpris.MediaPlayer2.rmpd /org/mpris/MediaPlayer2
```

rmpd also advertises itself over **mDNS/Zeroconf** so MPD clients on the local network can auto-discover the server.

### OpenSubsonic Music Sources

rmpd can aggregate a remote [OpenSubsonic](https://opensubsonic.netlify.app/)
server (Navidrome, Airsonic, gonic, …) into its library as a *music source*.
The remote catalog is synced into the database at startup and on `update`, and
tracks stream on demand through the same HTTP path used for internet radio — so
any MPD client browses and plays them like local files (under mount-style paths
such as `home/Artist/Album/<id>.flac`, where the source name is the mount point).

Build with the `subsonic` feature, then add one or more `[[source]]` blocks:

```bash
cargo build --release --features subsonic
```

```toml
[[source]]
name = "home"                      # becomes the mount point (top-level directory)
type = "subsonic"
enabled = true
url = "https://music.example.com"
username = "alice"
password = "secret"                # or use `api_key = "..."` instead
# max_bitrate = 320                 # optional server-side transcode cap (kbps)
# format = "mp3"                    # optional transcode target ("raw" = no transcode)
```

Credentials are never written to logs. An unreachable server is skipped at
startup without aborting (previously-synced tracks remain browsable).

### Multi-Room, HTTP Streaming & Snapcast

rmpd plays to **all enabled outputs simultaneously**, so local audio and a
network stream can run at once. Two routes to networked/multi-room playback:

- **HTTP streaming** — enable a `type = "httpd"` output (default port 8000) and
  point any browser, phone, or another MPD/VLC client at `http://<host>:8000`.
  Works today, no extra daemon required (`encoder = "wav"` or `"pcm"`). The same
  output also serves Shoutcast/Icecast clients (`ICY 200 OK` greeting plus
  interleaved `StreamTitle` metadata).
- **Snapcast (synchronized)** — enable a `type = "fifo"` output writing to
  `/tmp/snapfifo` and run an external [Snapcast](https://github.com/badaix/snapcast)
  `snapserver` reading that FIFO for sample-accurate multi-room sync.

## Status & Roadmap

### Implemented

- **Core**: MPD protocol server (TCP/Unix sockets), event bus, configuration management, logging via `tracing`
- **Library**: filesystem scanning + watcher, SQLite database, metadata/artwork extraction via `symphonia`, full-text search via `tantivy`
- **MPD protocol**: playback commands (play/pause/stop/seek), queue management (add/delete/move/shuffle), database queries (find/search/list), status/statistics, playlist management (`.m3u`, `.pls`, XSPF/ASX; `.cue` sheets expand into range-restricted virtual tracks), output control
- **Audio**: gapless playback, crossfade and MixRamp transitions, ReplayGain, internet radio input with Shoutcast/Icecast (ICY) "now playing" metadata — see [Format Support](#format-support) for codec coverage and [Integrations](#integrations) for multi-room, MPRIS, and OpenSubsonic
- **Network storage**: `mount`/`unmount` shell out to the system `mount(8)` for NFS and SMB/CIFS shares (Linux and macOS), exposed under the music directory like MPD's storage plugins — no in-process NFS/SMB client

### In Progress

- Compressed stream encoders (FLAC / Opus / Vorbis) for the `httpd` output

## Compatibility

### Tested MPD Clients

- ✅ **mpc** — command-line client
- ✅ **ncmpcpp** — TUI client
- ✅ **Cantata** — Qt GUI client
- ✅ **rmpc** — modern TUI client
- 🚧 **MPDroid** — Android client (testing in progress)
- 🚧 **MPDluxe** — iOS client (testing in progress)

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

GitHub Actions runs formatting/clippy/`cargo doc` checks, a cross-platform test
matrix (Ubuntu + macOS, stable + nightly), a compatibility suite (state
persistence, database compatibility, decoder validation), a dependency-pruning
lint (`cargo-machete`), coverage reporting (Codecov), and release builds for 3
targets (x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
aarch64-apple-darwin — no x86_64 macOS build). A separate workflow runs
`cargo-audit`/`cargo-deny` security audits, and Renovate keeps dependencies
current. See [CI.md](CI.md) for details.

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
- Built with modern Rust audio libraries: [Symphonia](https://github.com/pdeljanov/Symphonia), [cpal](https://github.com/RustAudio/cpal)
- Special thanks to the Rust audio community

## Links

- **Documentation**: [CI.md](CI.md) - CI/CD and development guide
- **MPD Protocol**: [MPD Protocol Documentation](https://mpd.readthedocs.io/en/latest/protocol.html)
- **Issue Tracker**: [GitHub Issues](https://github.com/M0Rf30/rmpd/issues)
- **Discussions**: [GitHub Discussions](https://github.com/M0Rf30/rmpd/discussions)
