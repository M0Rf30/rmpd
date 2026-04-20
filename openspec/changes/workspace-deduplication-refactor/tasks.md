## 1. Audio Output Consolidation (rmpd-player)

- [x] 1.1 Create `rmpd-player/src/conversion.rs` with `samples_to_s16le()`, `f32_to_i16()`, `f32_to_i32()` functions and unit tests
- [x] 1.2 Create `SampleBuffer<T>` struct in `conversion.rs` with channel-based refill logic, silence on underrun, and unit tests
- [x] 1.3 Refactor `AudioOutput` trait in `audio_output.rs`: add `fn paused(&self) -> &PauseState` method, provide default `pause()`, `resume()`, `is_paused()` implementations that delegate to a `PauseState` helper struct
- [x] 1.4 Update `FifoOutput` to use `conversion::samples_to_s16le()` and remove its private `samples_to_s16le()` method
- [x] 1.5 Update `PipeOutput` to use `conversion::samples_to_s16le()` and remove its private duplicate
- [x] 1.6 Update `RecorderOutput` to use `conversion::samples_to_s16le()` and remove inline conversion logic
- [x] 1.7 Update `CpalOutput` (output.rs) to use `SampleBuffer<T>` in its 3 cpal callback closures (F32, I16, I32 formats) and use `f32_to_i16()`/`f32_to_i32()` from conversion module
- [x] 1.8 Update `DopOutput` to use `SampleBuffer<T>` in its 2 cpal callback closures
- [x] 1.9 Update all 5 output backends (`FifoOutput`, `PipeOutput`, `RecorderOutput`, `CpalOutput`, `DopOutput`) to use default `pause()`/`resume()`/`is_paused()` from `AudioOutput` trait, removing their manual implementations
- [x] 1.10 Run `cargo test -p rmpd-player` and `cargo clippy -p rmpd-player` — verify all pass

## 2. Core Shared Utilities (rmpd-core)

- [x] 2.1 Create `rmpd-core/src/tag.rs` module: move `tag_fallback_chain()` from `filter.rs`, add `normalize_decimal()` from `rmpd-library/src/metadata.rs`, add `VORBIS_TAG_MAP` and `ITEM_KEY_TAG_MAP` constants
- [x] 2.2 Add `pub mod tag;` to `rmpd-core/src/lib.rs`
- [x] 2.3 Update `rmpd-core/src/filter.rs` to import `tag_fallback_chain` from `crate::tag` instead of defining it locally
- [x] 2.4 Update `rmpd-library/src/database.rs` to import `tag_fallback_chain` from `rmpd_core::tag` and remove local duplicate
- [x] 2.5 Update `rmpd-library/src/metadata.rs` to import `normalize_decimal`, `VORBIS_TAG_MAP`, `ITEM_KEY_TAG_MAP` from `rmpd_core::tag` and remove local copies
- [x] 2.6 Create `rmpd-core/src/time.rs` module: move `system_time_to_unix_secs()` from `rmpd-library/src/database.rs`, add `format_iso8601()` extracted from `rmpd-protocol/src/commands/utils.rs`
- [x] 2.7 Add `pub mod time;` to `rmpd-core/src/lib.rs`
- [x] 2.8 Update `rmpd-library/src/database.rs` to import `system_time_to_unix_secs` from `rmpd_core::time`
- [x] 2.9 Update `rmpd-protocol/src/commands/utils.rs` to import `format_iso8601` from `rmpd_core::time` and remove local `format_iso8601_timestamp()`
- [x] 2.10 Create `rmpd-core/src/path.rs` module: extract `expand_tilde()` from `config.rs` (make public), add `resolve_path()` extracted from `rmpd-protocol/src/commands/utils.rs`
- [x] 2.11 Add `pub mod path;` to `rmpd-core/src/lib.rs`
- [x] 2.12 Update `rmpd-core/src/config.rs` to import `expand_tilde` from `crate::path` instead of local private function
- [x] 2.13 Update `rmpd-protocol/src/commands/utils.rs` to import `resolve_path` from `rmpd_core::path` and remove local copy
- [x] 2.14 Add `tag_eq(&self, tag: &str, value: &str) -> bool` and `tag_contains(&self, tag: &str, value_lower: &str) -> bool` methods to `Song` impl in `rmpd-core/src/song.rs`
- [x] 2.15 Update `rmpd-protocol/src/commands/utils.rs` to use `Song::tag_eq()` and `Song::tag_contains()` instead of local `song_tag_eq()`/`song_tag_contains()` functions
- [x] 2.16 Run `cargo test --workspace` and `cargo clippy --workspace` — verify all pass

## 3. Protocol Handler Helpers (rmpd-protocol)

- [x] 3.1 Create `rmpd-protocol/src/helpers.rs` module with `pub(crate)` visibility
- [x] 3.2 Implement `update_playlist_version(state: &AppState)` in helpers.rs
- [x] 3.3 Replace all 13 inline playlist version update blocks in `commands/queue.rs` with calls to `helpers::update_playlist_version()`
- [x] 3.4 Implement `is_known_uri_scheme(scheme: &str) -> bool` in helpers.rs
- [x] 3.5 Replace both URI scheme validation blocks in `commands/queue.rs` (handle_add_command and handle_addid_command) with `helpers::is_known_uri_scheme()`
- [x] 3.6 Implement `create_stream_song(uri: &str) -> Song` in helpers.rs
- [x] 3.7 Replace both stream song creation blocks in `commands/queue.rs` with `helpers::create_stream_song()`
- [x] 3.8 Implement `update_player_state(state: &AppState, new_state: PlayerState)` in helpers.rs
- [x] 3.9 Replace all 8 status update + event emission patterns in `commands/playback.rs` and `queue_playback.rs` with `helpers::update_player_state()`
- [x] 3.10 Implement `extract_audio_format(song: &Song) -> Option<AudioFormat>` in helpers.rs
- [x] 3.11 Replace all 3 audio format extraction patterns in `commands/playback.rs` and `queue_playback.rs` with `helpers::extract_audio_format()`
- [x] 3.12 Extract shared filter parsing logic from `commands/database.rs` into a helper function, parameterized by `case_sensitive: bool`
- [x] 3.13 Update `handle_find_command` and `handle_search_command` to use the shared filter parsing helper
- [x] 3.14 Add `pub(crate) mod helpers;` to `rmpd-protocol/src/lib.rs`
- [x] 3.15 Run `cargo test -p rmpd-protocol` and `cargo clippy -p rmpd-protocol` — verify all pass

## 4. Test Infrastructure Consolidation

- [x] 4.1 Extend `rmpd-core/src/test_utils.rs` with `FixtureGenerator` struct, `AudioFormat` enum, and `sanitize_for_filename()` — merging both player and library implementations to support pattern-based and metadata-based generation
- [x] 4.2 Add `fixtures_dir(crate_name: &str)` and `get_fixture(crate_name: &str, filename: &str)` helpers to `rmpd-core/src/test_utils.rs`
- [x] 4.3 Add `rmpd-core` with `test-utils` feature as dev-dependency in `rmpd-protocol/Cargo.toml` (if not already present)
- [x] 4.4 Remove `make_test_song()` from `rmpd-protocol/tests/common/tcp_harness.rs` — replace calls with `rmpd_core::test_utils::create_test_song()`
- [x] 4.5 Remove `create_test_song()` from `rmpd-protocol/tests/common/state_helpers.rs` — replace calls with `rmpd_core::test_utils::create_test_song()`
- [x] 4.6 Add `rmpd-core` with `test-utils` feature as dev-dependency in `rmpd-library/Cargo.toml` (if not already present)
- [x] 4.7 Remove local `create_test_song()` from `rmpd-library/tests/common/rmpd_harness.rs` — replace calls with `rmpd_core::test_utils::create_test_song()`
- [x] 4.8 Remove local `create_test_song()` from `rmpd-library/tests/common/comparison.rs` — replace calls with `rmpd_core::test_utils::create_test_song()`
- [x] 4.9 Update `rmpd-player/tests/fixtures/generator.rs` to re-export from `rmpd_core::test_utils::FixtureGenerator` and remove local implementation
- [x] 4.10 Update `rmpd-library/tests/fixtures/generator/mod.rs` to re-export from `rmpd_core::test_utils::FixtureGenerator` and remove local implementation
- [x] 4.11 Update `rmpd-player/tests/fixtures/pregenerated.rs` and `rmpd-library/tests/fixtures/pregenerated.rs` to use shared `fixtures_dir()` / `get_fixture()` helpers
- [x] 4.12 Run `cargo test --workspace` — verify all tests still pass after migration

## 5. Error Handling Standardization

- [x] 5.1 Add `library-errors` feature to `rmpd-core/Cargo.toml` with optional dependencies on `lofty`, `tantivy`, `notify`
- [x] 5.2 Add `From<lofty::error::LoftyError>`, `From<tantivy::TantivyError>`, `From<notify::Error>` impls to `rmpd-core/src/error.rs` gated by `library-errors` feature
- [x] 5.3 Add `protocol-errors` feature to `rmpd-core/Cargo.toml` with optional dependency on `mdns-sd`
- [x] 5.4 Add `From<mdns_sd::Error>` impl to `rmpd-core/src/error.rs` gated by `protocol-errors` feature
- [x] 5.5 Enable `library-errors` feature in `rmpd-library/Cargo.toml` dependency on `rmpd-core`
- [x] 5.6 Replace straightforward `.map_err(|e| RmpdError::Library(e.to_string()))` calls in `rmpd-library/src/` with `?` operator using new `From` impls (keep context-specific map_err calls that add file paths or operation names)
- [x] 5.7 Enable `protocol-errors` feature in `rmpd-protocol/Cargo.toml` dependency on `rmpd-core`
- [x] 5.8 Replace `anyhow::Result` with `rmpd_core::error::Result` in `rmpd-protocol/src/server.rs`
- [x] 5.9 Replace `Result<T, anyhow::Error>` with `rmpd_core::error::Result<T>` in `rmpd-protocol/src/discovery.rs`
- [x] 5.10 Replace `anyhow::Result` with `rmpd_core::error::Result` in `rmpd-protocol/src/queue_playback.rs`
- [x] 5.11 Update `rmpd/src/app.rs` to return `rmpd_core::error::Result` from internal functions, converting to `anyhow::Error` only in `main()`
- [x] 5.12 Remove `anyhow` dependency from `rmpd-protocol/Cargo.toml`
- [x] 5.13 Run `cargo test --workspace` and `cargo clippy --workspace` — verify all pass

## 6. Command Dispatch Derive Macro

- [x] 6.1 Create `rmpd-macros/` crate directory with `Cargo.toml` (`[lib] proc-macro = true`), depending on `syn`, `quote`, `proc-macro2`
- [x] 6.2 Add `rmpd-macros` to workspace `Cargo.toml` members list
- [x] 6.3 Implement `#[derive(CommandMetadata)]` proc-macro that parses `#[command(name = "...", permission = ...)]` attributes and generates `command_name(&self) -> &'static str` and `command_required_permission(&self) -> u8` methods
- [x] 6.4 Add compile-time error for variants missing `#[command(name = "...")]` attribute
- [x] 6.5 Add `rmpd-macros` as dependency in `rmpd-protocol/Cargo.toml`
- [x] 6.6 Annotate all `Command` enum variants in `rmpd-protocol/src/parser.rs` with `#[command(name = "...", permission = ...)]` attributes matching current behavior
- [x] 6.7 Remove manual `command_name()` function (~105 match arms) from `rmpd-protocol/src/server.rs`
- [x] 6.8 Remove manual `command_required_permission()` function (~105 match arms) from `rmpd-protocol/src/server.rs`
- [x] 6.9 Write equivalence tests: iterate all `Command` variants and assert derived output matches a reference snapshot of the old manual implementation
- [x] 6.10 Run `cargo test --workspace` and `cargo clippy --workspace` — verify all pass
