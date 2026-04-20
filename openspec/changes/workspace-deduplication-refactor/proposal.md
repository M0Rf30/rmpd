## Why

The rmpd workspace contains ~1150 lines of duplicated code spread across 34 identified patterns in 7 crates. Core logic (tag fallback chains, sample format conversion, test song creation, playlist state updates) is copy-pasted rather than shared, meaning bug fixes and behavior changes must be applied in multiple locations. This increases maintenance burden and introduces subtle divergence risk as the project grows toward full MPD compatibility.

## What Changes

- Extract shared output trait defaults and audio sample conversion utilities in `rmpd-player` to eliminate 5 identical `pause/resume/is_paused` implementations and 3 duplicated `samples_to_s16le` functions
- Create new `rmpd-core` modules (`time.rs`, `path.rs`) and consolidate `tag_fallback_chain()` to centralize utilities currently duplicated across `rmpd-core`, `rmpd-library`, and `rmpd-protocol`
- Extract protocol command handler helpers (playlist version update, URI scheme validation, stream song factory, player state transitions) to reduce ~300 lines of boilerplate in `rmpd-protocol`
- Consolidate test infrastructure: unify 5 copies of test song creation functions and 2 copies of `FixtureGenerator`/`AudioFormat` into shared test utilities
- Standardize error handling: resolve `anyhow` vs `RmpdError` inconsistency across 5 files, add missing `From` impls to replace ~100 scattered `map_err` calls
- Introduce a `CommandMetadata` derive macro to eliminate 105+ match arms in `server.rs` command dispatch

## Capabilities

### New Capabilities
- `audio-output-consolidation`: Default trait implementations for `AudioOutput` and shared sample format conversion utilities in `rmpd-player`
- `core-shared-utilities`: New `rmpd-core` modules for timestamp conversion, path expansion, and consolidated tag fallback logic
- `protocol-handler-helpers`: Extracted helper functions for repeated protocol command patterns (playlist state, URI validation, stream songs, player state transitions)
- `test-infrastructure-consolidation`: Unified test fixtures, song creation, and fixture generators shared across all crates via `rmpd-core` test-utils feature
- `error-handling-standardization`: Consistent error strategy across the workspace with centralized `From` impls and resolved `anyhow`/`thiserror` boundary
- `command-dispatch-derive`: Derive macro for command metadata (name, permission) to replace manual match arms in `server.rs`

### Modified Capabilities

## Impact

- **Crates modified**: All 7 workspace crates (`rmpd`, `rmpd-core`, `rmpd-protocol`, `rmpd-player`, `rmpd-library`, `rmpd-plugin`, `rmpd-stream`)
- **Public API**: No external-facing changes. All refactoring is internal.
- **Dependencies**: New `proc-macro` crate may be needed for `CommandMetadata` derive macro (Phase 6), or alternatively a build script approach
- **Risk**: Each phase is independently shippable and testable. Phases can be landed incrementally.
- **Tests**: Existing test suite must pass after each phase. Test infrastructure changes (Phase 4) require careful migration to avoid breaking test isolation.
