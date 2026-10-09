<!--
SPDX-FileCopyrightText: 2026 Gianluca Boiano
SPDX-License-Identifier: MIT OR Apache-2.0
-->

# rmpd plugin architecture

rmpd follows MPD's *compile-time* plugin model, with Mopidy-style
configuration: every plugin category is a Rust **trait** plus a `const`
name→factory registry, selected at runtime by name from the config file and
gated at build time by **Cargo features**. There is deliberately **no dynamic
loading**: Rust has no stable ABI, and MPD itself links all plugins statically.

## Categories

| Category        | SPI trait / location                                  | Registry                         | Config                  |
|-----------------|-------------------------------------------------------|----------------------------------|-------------------------|
| Outputs         | `rmpd-player` (output SPI)                            | `OUTPUT_PLUGINS` (`rmpd-player`) | `[[output]]`            |
| Mixers          | `rmpd-player` (mixer SPI)                             | in `rmpd-player`                 | output block settings   |
| Encoders        | `rmpd-player` (encoder SPI)                           | in `rmpd-player`                 | output block settings   |
| DSP filters     | `rmpd_player::AudioFilter`                            | `FILTER_PLUGINS` (`rmpd-player`) | `[[filter]]` + `[audio].filters` / output `filters` |
| Music sources   | `rmpd_plugin::MusicSource`                            | `SOURCE_PLUGINS` (`rmpd-source`) | `[[source]]`            |
| Playlist parsers| `rmpd_plugin::PlaylistParser`                         | `PLAYLIST_PLUGINS` (`rmpd-plugin`)| none (by suffix / MIME)|
| Input schemes   | `rmpd_stream::InputPlugin`                            | `INPUT_PLUGINS` (`rmpd-stream`)  | `[stream]` (timeout, proxy, ICY blacklist, HLS bandwidth) |
| Integrations    | `rmpd_plugin::Integration`                            | `INTEGRATION_PLUGINS` (`rmpd-integrations`) | `[[integration]]` |
| Artwork providers | `rmpd_plugin::ArtworkProvider`                      | `ARTWORK_PLUGINS` (`rmpd-integrations`) | `[[artwork]]`     |

## Placement rule

* **Cross-cutting SPIs** (used by more than one subsystem, or by the protocol
  layer *and* a backend) live in `rmpd-plugin`. That crate depends only on
  `rmpd-core` plus tiny crates (`async-trait`, `tokio`, `tracing`).
* **Subsystem-local SPIs** (outputs, mixers, encoders) stay in `rmpd-player`
  next to the code that drives them; nothing outside the player needs them.
* **Implementations** never live in `rmpd-plugin`, except trivially pure ones
  (the built-in playlist parsers). Sources go in `rmpd-source`, integrations in
  `rmpd-integrations`, each behind its own Cargo feature when it pulls in a
  heavy dependency.
* Registries are `static` slices consulted by name; factories are synchronous
  and perform **no I/O** (I/O happens when a method is called).

### Mixer notes

* While any hardware mixer is active, `AppState::spawn_mixer_watch` polls it
  once per second; an externally changed level updates `status.volume` and
  emits `Event::VolumeChanged` (`idle mixer`, MPRIS `Volume`).
  `PlayerHandle::status().volume` (MPRIS) reads the effective mixer volume.
* Software gain is applied **per output** (`OutputGains`: each output gets a
  control block sharing pause/flush state with the engine but with its own
  gain), so hardware- and `none`-mixer outputs stay at unity even next to
  software-mixer outputs; no double attenuation.
* `urlhandlers` is derived from `rmpd_stream::url_handlers()`.

## Sources

`MusicSource` (`rmpd-plugin/src/source.rs`) required methods: `scheme`, `name`,
`ping`, `browse`, `list_all`, `search`, `resolve_stream_uri`. Default-bodied
extensions:

| Method                 | Default            | Purpose                                         |
|------------------------|--------------------|-------------------------------------------------|
| `cover_art(id)`        | `Ok(None)`         | remote cover art                                |
| `lookup(uri)`          | `Ok(None)`         | resolve one URI to a `Song` without a sync      |
| `playlists()`          | `Ok(vec![])`       | server-side playlist names                      |
| `playlist_items(name)` | `Err(NotFound)`    | songs of a server-side playlist                 |
| `is_live(song_id)`     | `false`            | unbounded stream (radio): no duration, no seek  |
| `sync_policy()`        | `SyncPolicy::Full` | `Full` mirrors `list_all` into the DB; `OnDemand` never calls `list_all` and `lsinfo` under the mount delegates to `browse` (`SourceRegistry::browse_on_demand`) |

Registry entries are `SourcePlugin { name, settings, factory }`.

## Playlist parsers

`PlaylistParser` (`rmpd-plugin/src/playlist.rs`): `name()`, `suffixes()`,
`mime_types()`, `parse(base_uri, content) -> Vec<PlaylistEntry>` where
`PlaylistEntry { uri, title: Option<String>, duration: Option<Duration> }`.
Built-ins: `m3u` (incl. extended M3U `#EXTINF`), `pls`, `xspf`, `asx`. Look up
with `parser_for_suffix`, `parser_for_mime`, `parser_by_name`. Stored-playlist
commands in `rmpd-protocol` call this registry.

## Input schemes

`InputPlugin` (`rmpd-stream/src/input.rs`): `name()`, `schemes()` (lowercase,
without `://`) and `open(uri, &OpenContext) -> io::Result<OpenedInput>`.
`OpenedInput { source: Box<dyn MediaSource>, title: Option<TitleHandle>,
extension_hint, uri }`. `rmpd_stream::open(uri)` dispatches through
`INPUT_PLUGINS` by URI scheme (`input_for_uri`, `is_input_uri`,
`url_handlers`); the decoder calls it for every `scheme://` path. Built-in:
`http` (`http`, `https`).

The HTTP plugin unwraps **radio playlists**: when the URL suffix or response
`Content-Type` selects a `PlaylistParser` (`parser_for_mime` first, audio MIME
types win over suffixes, otherwise `parser_for_suffix`), the body is fetched
(1 MiB cap), parsed, and each entry (relative ones resolved against the
playlist URL) is opened through the registry until one succeeds. Nesting is
limited to 3 levels (`MAX_PLAYLIST_DEPTH`).

**HLS** playlists (`#EXT-X-TARGETDURATION`, `#EXT-X-STREAM-INF`,
`#EXT-X-MEDIA-SEQUENCE`) are played by `rmpd-stream/src/hls.rs`. Playlist
parsing and variant selection are pure (`hls_playlist.rs`): a master playlist
yields the best audio-only variant (or the best one not above
`[stream] hls_max_bandwidth`, falling back to the lowest; an `EXT-X-MEDIA`
audio rendition replaces a video variant). A worker thread walks the media
playlist (live playlists start three segments from the edge and are reloaded
every target duration, half of it while nothing new is listed; `ENDLIST` ends
the stream), downloads segments (byte ranges, 3 attempts), decrypts
`METHOD=AES-128` segments with pure-Rust `aes` + `cbc` (`SAMPLE-AES` →
`Unsupported`) and converts them: ADTS-AAC and MP3 pass through with ID3 tags
stripped, fMP4 is `EXT-X-MAP` init + fragments (Symphonia's isomp4 reader
streams fragmented files), and MPEG-TS goes through the minimal demuxer in
`ts.rs` (PAT → PMT → first audio PID, `stream_type` 0x0F AAC/ADTS, 0x03/0x04
MPEG audio) that emits the elementary stream. The decoder reads the result
from a bounded prefetch channel (`HlsSource`, not seekable); the first segment
is fetched during `open` to pick the probe hint (`aac`/`mp3`/`mp4`).
`[stream]` settings are installed process-wide with
`rmpd_stream::configure` at startup: `timeout_ms` (default 5000),
`metadata_blacklist` (fnmatch globs matched against the stream URL and any
playlist it was unwrapped from; matching streams ignore ICY `StreamTitle`) and
`[stream.proxy]` (`url`, `username`, `password`).

## Integrations

Long-running background tasks (scrobblers, notifiers, remote bridges).
`Integration` (`rmpd-plugin/src/integration.rs`):

```rust
#[async_trait]
pub trait Integration: Send {
    fn name(&self) -> &str;
    async fn run(self: Box<Self>, ctx: IntegrationContext) -> Result<(), PluginError>;
}
pub struct IntegrationContext {
    pub events: broadcast::Receiver<Event>,   // global event bus subscription
    pub player: Arc<dyn PlayerHandle>,        // status snapshot + controls
    pub state_dir: PathBuf,                   // per-instance, already created
    pub shutdown: ShutdownSignal,             // `cancelled().await` / `is_shutdown()`
}
```

`PlayerHandle` offers `status()` (state, elapsed, duration, volume, current
song) and `play/pause/toggle/next/previous/stop/set_volume/seek`; the daemon
implements it as `rmpd_protocol::ServerPlayerHandle` over the live server
state. `run` MUST return promptly once `ctx.shutdown` fires; the daemon waits
up to five seconds. Errors are logged and never crash the daemon.

### Built-in integrations: MPRIS and mDNS

Linux MPRIS (`type = "mpris"`, D-Bus name `org.mpris.MediaPlayer2.rmpd`) and
mDNS/Zeroconf (`type = "mdns"`, advertises `_mpd._tcp`) are ordinary
integrations in `rmpd-integrations` (features `mpris`, `mdns`, default on).
They are enabled implicitly by the legacy `[network]` switches: at startup
`rmpd_integrations::synthesize_builtin` turns `media_controls` into an `mpris`
block, and `mdns_config` turns `zeroconf_enabled`/`zeroconf_name` into an
`mdns` block once the TCP listener is bound (the advertised port is only known
then). An explicit `[[integration]]` block of the same type suppresses the
synthesized one. MPRIS needs more than the basic controls, so `PlayerHandle`
has additive, default-bodied extras: `current_song_id`, `queue_len`, `options`
(`PlayerOptions`), `set_repeat/random/single`, `seek_relative`, `position`
(live), `music_dir`, `request_shutdown`.

### HTTP / WebSocket API (`http`, feature `http-api`)

`rmpd-integrations/src/http_api/` (axum on hyper/tokio, pure Rust, off by
default). Settings: `bind` (`ip:port`, default `127.0.0.1:6680`),
`allowed_origins`, `static_dir`, `token`. Layout: `rpc.rs` (JSON-RPC 2.0
dispatch, transport-independent, single + batch + notifications), `model.rs`
(Mopidy `Track`/`TlTrack`/`Ref`/`SearchResult` JSON), `events.rs`
(`EventMapper`: bus `Event` → Mopidy event name/payload; `run_pump` fans the
JSON out to WebSocket clients), `mod.rs` (settings, router, CORS/Origin/token
guard, static files).

* `POST /rmpd/rpc` and `/mopidy/rpc`; WebSocket `/rmpd/ws` and `/mopidy/ws`
  (same JSON-RPC plus pushed `{"event": ..}` messages).
* Methods: `core.playback.*` (play with optional `tlid`, pause, resume, stop,
  next, previous, seek in ms, get_state/time_position/current_track/
  current_tl_track/current_tlid), `core.mixer.get/set_volume`,
  `core.tracklist.*` (length, tl_tracks, tracks, add(uris, at_position),
  clear, index, random/repeat/single get+set), `core.library.browse/search`,
  `core.describe`. A Mopidy search query is flattened into one all-tags
  case-insensitive substring search.
* Events: `track_playback_started/paused/resumed/ended`,
  `playback_state_changed`, `volume_changed`, `tracklist_changed`,
  `options_changed`, `seeked` (inferred from position jumps), 
  `stream_title_changed`.
* Security: the token (bearer header, or `?token=` for WebSocket) protects the
  RPC/WebSocket paths; static files are public. A request with an `Origin`
  header gets 403 unless listed in `allowed_origins` (`*` = any) or equal to
  the request's `Host` when that is `localhost`/an IP literal (blocks
  cross-site WebSocket hijacking and DNS rebinding). Static paths are
  sanitised and canonicalised to stay under `static_dir`.

To serve this, `PlayerHandle` gained additive default-bodied methods
(`queue_entries`, `add_uris`, `clear_queue`, `play_id`, `browse`, `search`;
types `QueueEntry`, `BrowseEntry`/`BrowseKind`). `ServerPlayerHandle` routes
them through the existing `addid`/`add`/`clear`/`playid` handlers, the library
database (`lsinfo`-equivalent listing, `search any`) and on-demand sources.

**macOS Now Playing is intentionally *not* an integration.** AppKit's
`MPNowPlayingInfoCenter`/remote-command stack must be driven from the process
main thread's run loop, which owns the process, whereas integrations run as
Tokio tasks on worker threads. It stays in `rmpd-protocol`
(`media_controls_macos`) and is started from `main.rs`; `network.media_controls`
still controls it.

## DSP filters

`AudioFilter` (`rmpd-player/src/filter.rs`): `name()`, `apply(&mut [f32])`
(in place over interleaved samples) and the default-bodied
`set_format(sample_rate, channels)`, called before the first chunk and when the
format changes (sample-rate-dependent coefficients are recomputed there).
Filters keep the channel count. `FilterChain` composes them.

Registry entries are `FilterPlugin { name, settings, factory }` in the static
`FILTER_PLUGINS`; the factory takes `FilterParams { sample_rate, channels,
settings }` and returns `Box<dyn AudioFilter>` or `FilterError::Config`
(never echoing values). Built-ins live in `rmpd-player/src/dsp.rs`:

| Type                | Settings                                                                 |
|---------------------|--------------------------------------------------------------------------|
| `normalize`         | `target_db`, `max_gain_db`, `attack_ms`, `release_ms`, `window_ms`, `ceiling_db` — peak-envelope AGC, smoothed gain, instant-attack limiter |
| `equalizer`         | `bands = [{ freq, gain_db, q }]` (≤ 32 peaking RBJ biquads), `preamp_db` |
| `route` / `channels`| `mode` (`mono`, `swap`, `left`, `right`, `map`), `map`                   |

Configuration: `[[filter]]` blocks (`name`, `type`, `enabled`, settings) define
named instances. The chain of an output is its own `filters = ["a", "b"]`
setting (an empty list disables filtering) or else the global
`[audio].filters`. `rmpd_player::filter::configure` installs the definitions
process-wide at startup and returns warnings (duplicate names, unknown
types/keys, rejected settings, undefined references); the engine builds one
chain per output when the outputs are opened and runs it in that output's
worker (`MultiOutput::spawn_with_filters`) before the write-time volume. The
chain fingerprint is part of the `OutputSlot` reuse key, so changing filters
rebuilds the outputs while the default (no filters) path is untouched. A
filter that fails to build is skipped with a warning.

## Artwork providers

Cover-art fallback for `albumart` / `readpicture`. `ArtworkProvider`
(`rmpd-plugin/src/artwork.rs`): `name()`, `async fetch(&Song) -> Option<(Vec<u8>,
String)>` (image bytes + MIME) and the default-bodied
`fetch_outcome(&Song) -> ArtworkOutcome` (`Found` / `NotFound` /
`Unavailable`) which network providers override so a transient failure is not
cached like a definitive miss. Registry entries are `ArtworkPlugin { name,
settings, factory }` in `ARTWORK_PLUGINS` (`rmpd-integrations/src/artwork`),
built from `[[artwork]]` blocks (`name`, `type`, `enabled`, settings) by
`rmpd_integrations::build_artwork_resolver` into an `ArtworkResolver` (ordered
chain, first `Found` wins) stored in `AppState::artwork`.

Only songs with **no** cover file and **no** embedded picture reach the
providers (`rmpd-protocol/src/commands/database.rs::remote_artwork_for`;
`albumart` checks for an embedded picture first so it never fetches when one
exists). Source-backed (mounted) songs keep using their source's `cover_art`.
The cache lives in the artwork database, in a `remote_artwork` table keyed per
album (`artwork_cache_key`: `mbid:<MUSICBRAINZ_ALBUMID>` or
`album:<albumartist>\x1f<album>`, lowercased) so tracks share one entry and no
`songs` row is needed. Hits never expire; a definitive miss is remembered for
7 days (`NEGATIVE_TTL_SECS`), a transient failure for 10 minutes
(`TRANSIENT_TTL_SECS`).

Built-in provider `coverartarchive` (feature `coverart`, off by default;
settings `lookup`, `size`, `contact`, `caa_url`, `mb_url`): fetches
`<caa_url>/release/<MUSICBRAINZ_ALBUMID>/front-<size>` (then `release-group/`
with `MUSICBRAINZ_RELEASEGROUPID`); with `lookup = true` and no album MBID it
runs a MusicBrainz release search by album artist + album (score >= 90, one
request per second, `User-Agent: rmpd/<ver> ( <contact> )`). Responses are
capped at 5 MiB and must be images (magic bytes / `image/*`).

## Settings and unknown-key diagnostics

Plugin settings are flattened into the `[[source]]` / `[[integration]]` table
next to `name`, `type`, `enabled`. Each registry entry declares the keys it
accepts (`settings: &'static [&'static str]`). When a block is instantiated,
`rmpd_core::config::unknown_setting_messages` reports every other key as a
warning with a "did you mean" hint. Warnings never abort startup, and values
(possibly secrets) are never printed.

## Adding a plugin

1. **Pick the category and crate** using the placement rule above.
2. Implement the trait; keep the factory synchronous and I/O-free; return
   `SourceError::Config` / `PluginError::Config` for bad settings (without
   echoing secrets).
3. Declare `pub const SETTINGS: &[&str]` listing every accepted key.
4. Register it: add a `SourcePlugin` / `IntegrationPlugin` entry to the
   category's static registry (or the parser to `PLAYLIST_PLUGINS`). Gate heavy
   plugins with `#[cfg(feature = "...")]` and forward the feature through
   `rmpd/Cargo.toml`.
5. Add unit tests next to the plugin; registry tests cover unknown types.
6. Document user-facing settings in the example config.
