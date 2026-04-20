## Context

The rmpd workspace is a 7-crate Rust project implementing an MPD-compatible music server. It has grown organically with feature additions across `rmpd-core`, `rmpd-protocol`, `rmpd-player`, `rmpd-library`, `rmpd-plugin`, and `rmpd-stream`. The codebase is generally disciplined (clippy, rustfmt, CI), but cross-crate code sharing has lagged behind feature development.

Current state:
- `rmpd-core` already serves as the shared foundation with `Song`, `PlayerState`, `Queue`, `Event`, `Filter`, `Config`, and error types.
- `rmpd-core` already has a `test-utils` feature gate with `create_test_song()`, but downstream crates don't use it—they copy the function.
- `AudioOutput` trait in `rmpd-player` has no default method implementations, so all 5 output backends duplicate `pause/resume/is_paused`.
- `rmpd-protocol/src/server.rs` is the largest file (~1132 lines) with 105+ match arms in two command dispatch functions.
- Error handling mixes `anyhow::Result` in application/protocol layer and `RmpdError` (thiserror) in library crates.
- The existing feature-gated `From` impl pattern in `error.rs` (e.g., `#[cfg(feature = "database-errors")]`) is the established convention for optional error conversions.

## Goals / Non-Goals

**Goals:**
- Eliminate all identified code duplication (~1150 lines across 34 patterns)
- Establish `rmpd-core` as the single source of truth for shared utilities (tag logic, timestamps, paths)
- Make the `AudioOutput` trait provide sensible defaults so new output backends require less boilerplate
- Standardize error handling boundaries: `thiserror`-based `RmpdError` for all library/protocol code, `anyhow` only at the binary entry point (`main.rs`)
- Consolidate test infrastructure so all crates share fixtures via `rmpd-core`'s `test-utils` feature
- Each phase is independently shippable and testable—no phase depends on another being complete first

**Non-Goals:**
- Changing any external-facing behavior or MPD protocol compatibility
- Refactoring the overall crate architecture (module boundaries stay the same)
- Adding new features or capabilities
- Rewriting the parser or command dispatch from scratch (Phase 6 augments, not replaces)
- Achieving 100% `map_err` elimination—some context-specific error messages are better inline

## Decisions

### 1. Default trait methods on `AudioOutput` (not a base struct)

The `AudioOutput` trait currently has 6 methods with no defaults. Adding an `is_paused` field to the trait isn't possible in Rust, but we can restructure by having the trait require only `start`, `write`, and `stop` while providing default `pause`/`resume`/`is_paused` that operate on a new `PauseState` helper struct each backend embeds.

**Alternative considered**: A base struct with delegation. Rejected because Rust doesn't have inheritance—composition via a small embedded struct is idiomatic and avoids trait object complications.

### 2. New `rmpd-core` modules for shared utilities

Create `rmpd-core::tag` (tag fallback chains, normalize_decimal, tag maps), `rmpd-core::time` (timestamp conversions), and `rmpd-core::path` (tilde expansion, path resolution). These are new public modules in `rmpd-core/src/lib.rs`.

**Alternative considered**: A separate `rmpd-utils` crate. Rejected because the utilities are small, specific to rmpd's domain, and `rmpd-core` already serves this role.

### 3. Protocol helper module (`rmpd-protocol::helpers`)

Extract into a new `rmpd-protocol/src/helpers.rs` module: `update_playlist_version()`, `is_known_uri_scheme()`, `create_stream_song()`, `update_player_state()`. These are `pub(crate)` functions used only within the protocol crate.

**Alternative considered**: Putting these in the existing `commands/utils.rs`. Rejected because `commands/utils.rs` already handles response-level utilities—state mutation helpers are a different concern.

### 4. Shared sample conversion module in `rmpd-player`

Create `rmpd-player/src/conversion.rs` with `samples_to_s16le()`, `f32_to_i16()`, `f32_to_i32()`, and a generic `SampleBuffer<T>` for the cpal callback pattern. This stays in `rmpd-player` (not core) because it's audio-specific.

**Alternative considered**: Macros for the cpal callback closures. Rejected because a generic struct with type parameter is cleaner, more testable, and more debuggable than macro-generated code.

### 5. Error boundary: `anyhow` at binary edge only

Standardize: `RmpdError` (thiserror) everywhere except `rmpd/src/main.rs` which converts at the boundary via `anyhow::Error::from(RmpdError)`. Add new feature-gated `From` impls in `error.rs` for `lofty::Error`, `tantivy::TantivyError`, and `notify::Error`. The `rmpd-protocol` crate switches from `anyhow::Result` to `rmpd_core::error::Result`.

**Alternative considered**: Keep `anyhow` in the protocol layer. Rejected because the protocol layer needs typed errors for ACK response generation—`anyhow` erases the error type.

### 6. Attribute macro approach for command dispatch (Phase 6)

Use a proc-macro crate (`rmpd-macros`) to derive `CommandMetadata` on the `Command` enum, generating `command_name()` and `command_required_permission()` from attributes. This replaces 105+ manual match arms.

**Alternative considered**: Build script code generation. Rejected because proc-macros integrate naturally with Rust's derive system and the enum is already defined in the protocol crate. Also considered: `strum` crate for string mapping—rejected because we also need permission mapping which `strum` doesn't cover.

## Risks / Trade-offs

- **[Compile time]** Adding a proc-macro crate (Phase 6) increases compile time for the protocol crate. → Mitigation: Phase 6 is last and optional. The macro crate is tiny.
- **[Test isolation]** Sharing fixtures via `rmpd-core` couples test infrastructure. → Mitigation: Everything is behind `#[cfg(feature = "test-utils")]` and `#[cfg(test)]`. Dev-dependency only.
- **[Churn]** Touching many files across crates increases merge conflict risk. → Mitigation: Each phase is a self-contained PR. Phases 1-3 can land in any order.
- **[Feature flag complexity]** Adding more feature flags to `rmpd-core` for error conversions. → Mitigation: Follow the existing pattern (`database-errors`, `player-errors`). Add `library-errors`, `protocol-errors` symmetrically.
- **[SampleBuffer generic]** The generic buffer abstraction may not fit all future output backends. → Mitigation: Keep it simple (single struct, no trait hierarchy). Backends can opt out and implement custom logic.
