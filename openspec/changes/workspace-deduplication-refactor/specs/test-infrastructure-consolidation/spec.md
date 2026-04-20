## ADDED Requirements

### Requirement: All crates use rmpd-core test song creation
All workspace crates SHALL use `rmpd_core::test_utils::create_test_song()` and `rmpd_core::test_utils::create_test_song_with_metadata()` for test song creation, removing local duplicates.

#### Scenario: Protocol test harness uses core test songs
- **WHEN** `rmpd-protocol/tests/common/tcp_harness.rs` needs to create test songs
- **THEN** it SHALL call `rmpd_core::test_utils::create_test_song()` instead of its local `make_test_song()` function

#### Scenario: Protocol state helpers use core test songs
- **WHEN** `rmpd-protocol/tests/common/state_helpers.rs` needs to create test songs
- **THEN** it SHALL call `rmpd_core::test_utils::create_test_song()` instead of its local `create_test_song()` function

#### Scenario: Library harness uses core test songs
- **WHEN** `rmpd-library/tests/common/rmpd_harness.rs` needs to create test songs
- **THEN** it SHALL call `rmpd_core::test_utils::create_test_song()` instead of its local duplicate

#### Scenario: Library comparison module uses core test songs
- **WHEN** `rmpd-library/tests/common/comparison.rs` needs to create test songs
- **THEN** it SHALL call `rmpd_core::test_utils::create_test_song()` instead of its local duplicate

### Requirement: Unified fixture generator
The `rmpd-core` crate SHALL provide a shared `FixtureGenerator` and `AudioFormat` enum behind the `test-utils` feature, merging the two near-identical implementations from `rmpd-player` and `rmpd-library`.

#### Scenario: Player tests use shared fixture generator
- **WHEN** `rmpd-player` integration tests need to generate audio fixtures
- **THEN** they SHALL use `rmpd_core::test_utils::FixtureGenerator` instead of their local implementation

#### Scenario: Library tests use shared fixture generator
- **WHEN** `rmpd-library` integration tests need to generate audio fixtures with metadata
- **THEN** they SHALL use `rmpd_core::test_utils::FixtureGenerator` with metadata options instead of their local implementation

#### Scenario: Fixture generator supports both player and library needs
- **WHEN** the shared `FixtureGenerator` is configured
- **THEN** it SHALL support both pattern-based generation (sine_440hz, silence) for player tests and metadata-based generation (title, artist, album, artwork) for library tests

### Requirement: Shared fixture path helpers
The `rmpd-core` crate SHALL provide `fixtures_dir()` and `get_fixture()` helper functions behind the `test-utils` feature, replacing duplicated implementations in player and library test fixtures.

#### Scenario: Consistent fixture directory resolution
- **WHEN** `fixtures_dir(crate_name: &str)` is called
- **THEN** it SHALL return the path to the fixtures directory for the specified crate (e.g., `rmpd-player/tests/fixtures/`)

#### Scenario: Fixture file lookup
- **WHEN** `get_fixture(crate_name: &str, filename: &str)` is called
- **THEN** it SHALL return the full path to the specified fixture file within that crate's fixtures directory

### Requirement: Shared sanitize_for_filename utility
The `rmpd-core` test utilities SHALL provide a single `sanitize_for_filename(name: &str) -> String` function, replacing the two identical implementations.

#### Scenario: Filename sanitization
- **WHEN** `sanitize_for_filename("My Song (Live)")` is called
- **THEN** it SHALL return a filesystem-safe string with special characters replaced
