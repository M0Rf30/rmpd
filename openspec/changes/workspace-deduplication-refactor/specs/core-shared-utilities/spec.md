## ADDED Requirements

### Requirement: Centralized tag utility module
The `rmpd-core` crate SHALL provide a `tag` module containing `tag_fallback_chain()`, `normalize_decimal()`, and tag mapping constants, accessible to all workspace crates.

#### Scenario: Tag fallback chain resolution
- **WHEN** `tag_fallback_chain(tag_name: &str)` is called with a tag name like "albumartist"
- **THEN** it SHALL return the ordered list of fallback tags as defined by MPD semantics (e.g., "albumartist" falls back to "artist")

#### Scenario: Decimal normalization for track/disc numbers
- **WHEN** `normalize_decimal(value: &str)` is called with a string like "03" or "3/12"
- **THEN** it SHALL return the normalized integer string (e.g., "3")

#### Scenario: Vorbis tag mapping
- **WHEN** the `VORBIS_TAG_MAP` constant is accessed
- **THEN** it SHALL provide the mapping from Vorbis comment field names to canonical MPD tag names

### Requirement: Centralized timestamp utility module
The `rmpd-core` crate SHALL provide a `time` module containing timestamp conversion functions used across the workspace.

#### Scenario: SystemTime to Unix seconds conversion
- **WHEN** `system_time_to_unix_secs(time: SystemTime)` is called
- **THEN** it SHALL return the number of seconds since Unix epoch as `i64`

#### Scenario: Unix seconds to ISO 8601 formatting
- **WHEN** `format_iso8601(unix_secs: i64)` is called
- **THEN** it SHALL return an RFC 3339 / ISO 8601 formatted string (e.g., "2024-01-15T10:30:00Z")

### Requirement: Centralized path utility module
The `rmpd-core` crate SHALL provide a `path` module containing path manipulation functions used across the workspace.

#### Scenario: Tilde expansion
- **WHEN** `expand_tilde(path: &str)` is called with a path starting with "~"
- **THEN** it SHALL replace the leading "~" with the user's home directory path

#### Scenario: Path resolution against music directory
- **WHEN** `resolve_path(relative: &str, music_dir: Option<&str>)` is called with a relative path
- **THEN** it SHALL resolve the path against the music directory if provided, or return it unchanged

### Requirement: Consumers use centralized utilities
The `rmpd-library` and `rmpd-protocol` crates SHALL import tag, time, and path utilities from `rmpd-core` instead of maintaining local copies.

#### Scenario: rmpd-library uses core tag_fallback_chain
- **WHEN** `rmpd-library/src/database.rs` needs tag fallback resolution
- **THEN** it SHALL call `rmpd_core::tag::tag_fallback_chain()` instead of its local duplicate

#### Scenario: rmpd-protocol uses core timestamp formatting
- **WHEN** `rmpd-protocol/src/commands/utils.rs` needs to format timestamps
- **THEN** it SHALL call `rmpd_core::time::format_iso8601()` instead of its local implementation

### Requirement: Song tag comparison methods
The `Song` struct in `rmpd-core` SHALL provide `tag_eq()` and `tag_contains()` methods for tag value matching, replacing protocol-layer utility functions.

#### Scenario: Exact tag match
- **WHEN** `song.tag_eq("artist", "Bach")` is called
- **THEN** it SHALL return `true` if the song has an "artist" tag with value exactly equal to "Bach" (case-insensitive)

#### Scenario: Substring tag match
- **WHEN** `song.tag_contains("title", "sonata")` is called
- **THEN** it SHALL return `true` if the song has a "title" tag whose lowercase value contains "sonata"
