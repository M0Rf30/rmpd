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
- ⚙️ **systemd Native** — `Type=notify` readiness and socket activation, like MPD; see [systemd](#systemd)
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

Audio fingerprinting (`getfingerprint`) uses
[chromaprint-next](https://github.com/attilagyorffy/chromaprint-next), a pure-Rust
Chromaprint port, so no C library or `cmake` is needed.

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
| `opus`                         | ✅ | ✅ — pure-Rust Opus decoder (SILK/CELT/hybrid) |
| `wav`                          | ✅ | ✅ |
| `aiff`, `aif`                  | ✅ | ✅ |
| `m4a`                          | ✅ | ✅ |
| `aac`                          | ✅ | ✅ |
| `ape` (Monkey's Audio)         | ✅ | ✅ — pure-Rust decoder |
| `wv` (WavPack)                 | ✅ | ✅ — pure-Rust decoder |
| `dsf`, `dff` (DSD)             | ✅ | ✅ — see [DSD](#dsd) |
| `mka`, `webm`                  | ✅ | ✅ |
| `mpc` (Musepack SV7/SV8)       | ✅ | ✅ — pure-Rust decoder |
| `wave`, `mp4`, `alac`, `caf`    | ❌ (not scanned into the library) | ✅ — playable if referenced directly |

### DSD

- `.dsf`/`.dff`, DSD64 through DSD256 and higher, all scan, tag, and play.
- DoP (DSD over PCM) sends a native DSD bitstream to a bit-perfect DAC over a raw ALSA `hw:` device. Opt in with `audio.dop = "yes"` (or `"auto"` to use DoP only when `audio.device` is set) or `RMPD_DOP=1`. DSD64, DSD128 and DSD256 are all supported; DoP always needs a PCM rate of `dsd_rate / 16` (176.4kHz for DSD64, 705.6kHz for DSD256), so a DAC that cannot accept that rate falls back to PCM automatically.
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
default, so a partial config is valid. Path values expand a leading `~`,
`$HOME`, and MPD's `$XDG_CONFIG_HOME`, `$XDG_MUSIC_DIR`, `$XDG_CACHE_HOME`,
`$XDG_DATA_HOME`, `$XDG_STATE_HOME` and `$XDG_RUNTIME_DIR`.

The state file defaults to `$XDG_STATE_HOME/rmpd/state` (usually
`~/.local/state/rmpd/state`; `$STATE_DIRECTORY` wins under systemd). An
existing state file at the old default, `~/.config/rmpd/state`, keeps being
used until the new one exists.

A minimal config:

```toml
[general]
music_directory = "~/Music"
log_level = "info"

[network]
bind_address = "127.0.0.1"
port = 6600
# A UNIX socket for local clients, in addition to TCP:
# unix_socket = "/run/user/1000/rmpd.sock"   # mpc -h /run/user/1000/rmpd.sock status
# Socket-only daemon. Either name the socket as the address, the way MPD's
# `bind_to_address` accepts a path...
# bind_address = "/run/user/1000/rmpd.sock"
# ...or empty the address and keep the separate key:
# bind_address = ""
# unix_socket = "/run/user/1000/rmpd.sock"

[audio]
default_output = "alsa"
replay_gain = "auto"
```

See [rmpd.toml](rmpd.toml) for a complete, annotated configuration example.

Additional `[general]`/`[network]` keys (see [rmpd.toml](rmpd.toml) for full examples):

- `state_file_interval` — seconds between periodic state-file saves; `0` disables periodic saving (default 120)
- `log_file` — write logs to this file instead of stdout (default: stdout)
- `max_playlist_length` — maximum number of songs in the queue (default 16384)
- `history_length` — number of recently played songs remembered for the HTTP API's `core.history.get_history`, kept across restarts in the state file; `0` disables it (default 1000)
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

### Command line

```bash
rmpd -c base.toml -c ~/.config/rmpd/local.toml   # layered; later files win (also `a.toml:b.toml` or a directory)
rmpd -o network.port=6601 -o audio.replay_gain=off   # override any key (Mopidy-style `section/key` also works)
rmpd config          # print the effective config, secrets masked
rmpd deps            # version, enabled features and compiled-in plugins
rmpd --kill          # ask the running instance to shut down (MPD `--kill`)
rmpd -q | -v | -vv   # warn / debug / trace logging; --stdout / --stderr override log_file
```

Logging to `log_file` follows MPD's log-rotation convention: send `SIGHUP`
and rmpd closes and re-opens the file, so `logrotate` can rename it first. A
failed re-open keeps the previous file open and logs a warning. The `SIGHUP`
handler is only installed when rmpd actually logs to a file (not for
stdout/stderr/journald) and is a no-op on non-Unix platforms.

```
/var/log/rmpd/rmpd.log {
    weekly
    rotate 4
    compress
    missingok
    postrotate
        systemctl kill -s HUP rmpd.service   # or: kill -HUP "$(pidof rmpd)"
    endscript
}
```

## Integrations

### MPRIS & mDNS

rmpd exposes a native [MPRIS](https://specifications.freedesktop.org/mpris-spec/latest/) interface on the session D-Bus as `org.mpris.MediaPlayer2.rmpd`. This lets desktops with a session bus (GNOME Shell, KDE Plasma), `playerctl`, lock screens, and multimedia keys discover and control rmpd directly — no external bridge such as `mpDris2` required. It is enabled by default and can be toggled with `media_controls` under `[network]` (the previous name, `mpris`, is still accepted).

On **macOS** the same setting enables the native Now Playing integration: rmpd appears in Control Center and on the lock screen, and hardware remote commands (play/pause, next and previous) control playback directly. `--daemonize` disables it there, since a detached daemon has no AppKit session.


On macOS, playback also auto-pauses when the default output device disappears while playing (for example a Bluetooth headset powering off), matching the platform convention of other media players. Disable it with `pause_on_device_loss = false` under `[audio]`.

```bash
playerctl -p rmpd metadata
busctl --user introspect org.mpris.MediaPlayer2.rmpd /org/mpris/MediaPlayer2
```

rmpd also advertises itself over **mDNS/Zeroconf** so MPD clients on the local network can auto-discover the server.

Both are implemented as built-in [integration plugins](docs/PLUGIN_ARCHITECTURE.md) (`mpris`, `mdns`; Cargo features of `rmpd-integrations`, on by default). The `[network]` switches `media_controls`, `zeroconf_enabled` and `zeroconf_name` keep working and implicitly enable them; you can also configure them explicitly with `[[integration]] type = "mpris"` / `type = "mdns"` (explicit blocks replace the implicit ones).

### Scrobbling & Webhooks

Three opt-in [integration plugins](docs/PLUGIN_ARCHITECTURE.md) (pure Rust, off by default; enable with `cargo build --release --features listenbrainz,lastfm,webhook`):

- **`listenbrainz`** — `token` (from <https://listenbrainz.org/settings/>), optional `api_url`. Sends *playing now* and listens, including MusicBrainz IDs from your tags.
- **`lastfm`** — `api_key`, `api_secret`, `session_key`; optional `api_url` (`https://libre.fm/2.0/` for libre.fm). Signed `track.updateNowPlaying` / `track.scrobble`.
- **`webhook`** — `url`, optional `events` filter, `headers`, and `secret` (adds `X-Rmpd-Signature: sha256=<HMAC-SHA256 of the body>`). POSTs `{"event", "timestamp", "payload"}` through a bounded, non-blocking queue.

A listen is submitted once a track of at least 30 s has been *actually played* for `min(duration/2, 4 min)`; paused time and seeks don't count. Listens that can't be delivered are kept in a JSONL queue under the state directory and retried with exponential backoff (cap: `max_queue`, default 10000). `scrobble_streams = true` also scrobbles radio streams from their ICY `Artist - Title`.

To get a Last.fm `session_key` (web auth flow): create an API account at <https://www.last.fm/api/account/create>; open `https://ws.audioscrobbler.com/2.0/?method=auth.getToken&api_key=KEY&format=json` to get a `TOKEN`; authorise it at `https://www.last.fm/api/auth/?api_key=KEY&token=TOKEN`; compute `SIG` with `echo -n 'api_keyKEYmethodauth.getSessiontokenTOKENSECRET' | md5sum` and open `https://ws.audioscrobbler.com/2.0/?method=auth.getSession&api_key=KEY&token=TOKEN&api_sig=SIG&format=json`. The returned `session.key` does not expire. Secrets are never logged.

### HTTP / WebSocket API

Opt-in integration (`--features http-api`, pure Rust on axum) exposing a Mopidy-compatible JSON-RPC 2.0 API so Mopidy web clients and scripts can drive rmpd. Settings: `bind` (default `127.0.0.1:6680`), `allowed_origins`, `static_dir` (serves a web client at `/`), `token` (optional bearer auth for the API).

- `POST /rmpd/rpc` (alias `/mopidy/rpc`): `core.playback.{play,pause,resume,stop,next,previous,seek,get_state,get_time_position,get_current_track,get_current_tl_track}`, `core.mixer.{get_volume,set_volume}`, `core.tracklist.{get_length,get_tl_tracks,get_tracks,add,clear,index,get_/set_random,repeat,single}`, `core.library.{browse,search}`, `core.history.{get_history,get_length}`, `core.describe`. `core.history.get_history` returns `[[timestamp_ms, Ref], ...]` for the most recently started songs, newest first (see `history_length`).
- `GET /rmpd/ws` (alias `/mopidy/ws`): the same JSON-RPC over WebSocket, plus pushed events: `track_playback_started/paused/resumed/ended`, `playback_state_changed`, `volume_changed`, `tracklist_changed`, `options_changed`, `seeked`, `stream_title_changed`.

```sh
curl -s localhost:6680/rmpd/rpc -d '{"jsonrpc":"2.0","id":1,"method":"core.playback.get_state"}'
```

### systemd

rmpd speaks systemd's protocols natively, without linking libsystemd:

- **Readiness** — the units use `Type=notify`; rmpd sends `READY=1` once every
  listener is bound and `STOPPING=1` on shutdown. Don't pass `--daemonize`
  under systemd (rmpd ignores it there).
- **Socket activation** — with `rmpd.socket` enabled, systemd owns the TCP
  and Unix sockets and starts rmpd on the first connection. Like MPD, rmpd
  then serves the passed sockets and ignores `bind_address`, `port` and
  `unix_socket`.

Units live in [`contrib/systemd`](contrib/systemd): `rmpd.service` and
`rmpd.socket` for the user instance, and `system/` for a system-wide
instance running as the `rmpd` user.

```bash
install -Dm644 contrib/systemd/rmpd.{service,socket} -t ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now rmpd.service   # or: rmpd.socket for on-demand start
```

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

### Jellyfin, Podcast & Radio Sources

Three more pure-Rust (reqwest/rustls, no C libraries) music sources, each behind
its own Cargo feature: `jellyfin`, `podcast` and `radio` (which also provides
`somafm`). Enable them with e.g. `cargo build --release --features jellyfin,podcast,radio`.

- **`jellyfin`** — like Subsonic: the catalog is synced into the database.
  Authenticates with `username`/`password` (`/Users/AuthenticateByName`) or an
  `api_key` (+ `user_id`); `max_bitrate`/`format` request `/Audio/{id}/universal`
  transcoding, otherwise the original file streams. Cover art and server-side
  playlists are supported.
- **`podcast`** — on-demand: `feeds` lists RSS URLs and/or OPML files; each
  podcast is a directory of episodes (newest first, `max_episodes`), with
  `itunes:image` cover art. Feeds are cached for `cache_ttl` seconds.
- **`radio`** — on-demand [Radio-Browser](https://www.radio-browser.info/)
  tree (favourites from `stations`, top voted/clicked, by country, by tag) plus
  `search`; **`somafm`** lists SomaFM channels. Entries are live streams.

```toml
[[source]]
name = "podcasts"
type = "podcast"
feeds = ["https://example.com/feed.xml", "~/subscriptions.opml"]

[[source]]
name = "radio"
type = "radio"
```

See `rmpd.toml` for every setting.

### Internet Radio (`[stream]`)

`http://` and `https://` URLs play as radio streams with Shoutcast/Icecast
(ICY) "now playing" titles. Playlist URLs — `.pls`, `.m3u`/`.m3u8`, `.asx`,
`.xspf`, or anything served with a matching `Content-Type` — are fetched (1 MiB
cap), unwrapped, and their entries tried in order until one opens (nesting is
limited to 3 levels). HLS (adaptive) playlists play natively: a master playlist
is reduced to one variant (audio-only preferred, capped by `hls_max_bandwidth`
when set), live playlists are reloaded at their target duration, and segments
may be ADTS-AAC, MP3, fragmented MP4 or MPEG-TS (the audio stream is extracted
in pure Rust); `AES-128` encrypted playlists are decrypted, `SAMPLE-AES` is
rejected with a clear error.

```toml
[stream]
timeout_ms = 5000                                  # connect / per-read timeout
hls_max_bandwidth = 128000                         # optional HLS ceiling, bits/s
metadata_blacklist = ["*://ads.example.com/*"]     # fnmatch globs: ignore ICY titles

[stream.proxy]                                     # optional HTTP proxy
url = "http://proxy.example.com:3128"
username = "alice"                                 # optional
password = "s3cr3t"                                # optional
```

### Cover Art Providers (`[[artwork]]`)

`albumart` serves a `cover.*` file next to the song and `readpicture` the
embedded picture. When a song has neither, rmpd can ask online
[artwork plugins](docs/PLUGIN_ARCHITECTURE.md) (pure Rust, off by default;
enable with `cargo build --release --features coverart`). `coverartarchive`
fetches `https://coverartarchive.org/release/<MUSICBRAINZ_ALBUMID>/front-500`;
with `lookup = true` it also finds songs lacking that tag through a MusicBrainz
search on album artist + album (limited to one request per second, with a
descriptive `User-Agent`). Results are cached per album in the database — hits
permanently, "no art" for 7 days, transient failures for 10 minutes.

```toml
[[artwork]]
name = "caa"
type = "coverartarchive"
lookup = true                  # MusicBrainz search when MUSICBRAINZ_ALBUMID is missing
size = "500"                   # 250, 500, 1200 or "original"
contact = "you@example.org"    # added to the User-Agent
```


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
- **Encoders** — `httpd`, `recorder` and `shout` outputs take an `encoder`
  setting from the compile-time encoder registry: `wav` (default), `pcm`,
  `flac` (pure Rust, streamable native FLAC; tune with `compression = 0..8`),
  or `opus` (pure Rust Ogg Opus, `audio/ogg`; `bitrate` in kbps, default 128,
  `complexity` 0..10, default 9, `vbr = "vbr" | "cvbr" | "cbr"`). Opus runs at
  48 kHz: other rates are resampled and multichannel input is folded to
  stereo. Use `opus` for Icecast/`shout` when bandwidth matters.
- **Icecast source (`shout`)** — a `type = "shout"` output pushes the encoded
  stream to an Icecast2 server over an HTTP `PUT` source connection
  (`host`, `port`, `mount`, `user` = `source`, `password`, `name`, `genre`,
  `description`, `public`, `encoder`) and updates the title via
  `/admin/metadata` on every song change. Plain HTTP only (no TLS); the
  default `flac` encoder needs a server/mount that accepts `audio/flac`;
  `encoder = "opus"` publishes an Ogg Opus mount (`audio/ogg`) at a fraction of
  the bandwidth.

### Volume & Mixers

Each `[[output]]` selects its volume control with `mixer_type`, like MPD:

| `mixer_type` | Behaviour |
|---|---|
| `software` (default) | rmpd's own gain stage (unchanged behaviour) |
| `hardware` | the output's native mixer — ALSA simple mixer for `cpal`/`alsa` outputs on Linux; build with `--features alsa-mixer` (reuses the `alsa` crate cpal already links, no extra native dependency). The software gain stays at 100%. |
| `none` / `null` | no volume control: `setvol`/`volume` fail with `problems setting volume`, and `status`/`getvol` omit `volume` |

Hardware mixers accept `mixer_device` (default: the card of the output's `hw:`
device, else `default`), `mixer_control` (default: `PCM`, then `Master`) and
`mixer_index` (default `0`). The reported volume comes from the active mixer;
an unavailable mixer type logs a warning and falls back to `software`. With
several outputs, `setvol` is applied to every mixer and `status` reports the
average; the software gain is shared by all outputs, so mixing software and
hardware outputs attenuates the hardware ones twice.

### DSP Filters

Mopidy/MPD-style [filter plugins](docs/PLUGIN_ARCHITECTURE.md), pure Rust and
off by default. Define named `[[filter]]` blocks (`name`, `type`, settings) and
pick the chain globally with `[audio].filters = ["a", "b"]` or per output with
`filters = [...]` (an empty list disables filtering for that output). Filters
run in order on the decoded stream, per output, before the software volume, and
keep the channel count.

| `type` | Purpose | Settings |
|---|---|---|
| `normalize` | AGC / volume normalization with smoothing and a limiter | `target_db` (-3), `max_gain_db` (30), `attack_ms` (10), `release_ms` (1500), `window_ms` (400), `ceiling_db` (-0.3) |
| `equalizer` | N-band parametric EQ (peaking biquads, recomputed per sample rate) | `bands = [{ freq, gain_db, q }, ...]`, `preamp_db` |
| `route` (alias `channels`) | stereo→mono downmix, L/R swap, channel remap | `mode` = `mono` \| `swap` \| `left` \| `right` \| `map`, `map = [...]` |

```toml
[audio]
filters = ["eq"]

[[filter]]
name = "eq"
type = "equalizer"
preamp_db = -3.0
bands = [{ freq = 60, gain_db = 4.0, q = 0.7 }, { freq = 8000, gain_db = 3.0 }]
```

Unknown filter types/settings and undefined filter names are reported at
startup as warnings; a filter that cannot be built is skipped. This is separate
from `audio.volume_normalization`, which only limits ReplayGain-boosted peaks.

## Status & Roadmap

### Implemented

- **Core**: MPD protocol server (TCP/Unix sockets), event bus, configuration management, logging via `tracing`
- **Library**: filesystem scanning + watcher, SQLite database (with FTS5 full-text search), metadata/artwork extraction via `symphonia`
- **MPD protocol**: playback commands (play/pause/stop/seek), queue management (add/delete/move/shuffle), database queries (find/search/list), status/statistics, playlist management (`.m3u`, `.pls`, XSPF/ASX; `.cue` sheets expand into range-restricted virtual tracks, reported with `RealUri`), output control, stickers on songs, playlists, tags and filters
- **Audio**: gapless playback, crossfade and MixRamp transitions, ReplayGain, internet radio input with Shoutcast/Icecast (ICY) "now playing" metadata — see [Format Support](#format-support) for codec coverage and [Integrations](#integrations) for multi-room, MPRIS, and OpenSubsonic
- **Network storage**: `mount`/`unmount` shell out to the system `mount(8)` for NFS and SMB/CIFS shares (Linux and macOS), exposed under the music directory like MPD's storage plugins — no in-process NFS/SMB client

### In Progress

- Ogg Vorbis stream encoder for `httpd`/`shout`/`recorder`: no production-ready
  pure-Rust encoder exists yet, and rmpd avoids C bindings. `wav`, `pcm`,
  native pure-Rust `flac` and `opus` encoders are available today.

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
