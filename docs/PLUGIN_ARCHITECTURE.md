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
| Music sources   | `rmpd_plugin::MusicSource`                            | `SOURCE_PLUGINS` (`rmpd-source`) | `[[source]]`            |
| Playlist parsers| `rmpd_plugin::PlaylistParser`                         | `PLAYLIST_PLUGINS` (`rmpd-plugin`)| none (by suffix / MIME)|
| Input schemes   | `rmpd-stream` (`http`, `https`, ...)                  | in `rmpd-stream`                 | none (by URI scheme)    |
| Integrations    | `rmpd_plugin::Integration`                            | `INTEGRATION_PLUGINS` (`rmpd-integrations`) | `[[integration]]` |

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
