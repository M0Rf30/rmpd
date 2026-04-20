## ADDED Requirements

### Requirement: RmpdError used throughout protocol crate
The `rmpd-protocol` crate SHALL use `rmpd_core::error::Result` (backed by `RmpdError`) for all public and internal function return types, replacing `anyhow::Result` usage.

#### Scenario: Server module uses RmpdError
- **WHEN** `rmpd-protocol/src/server.rs` functions return errors
- **THEN** they SHALL return `rmpd_core::error::Result<T>` instead of `anyhow::Result<T>`

#### Scenario: Discovery module uses RmpdError
- **WHEN** `rmpd-protocol/src/discovery.rs` functions return errors
- **THEN** they SHALL return `rmpd_core::error::Result<T>` instead of `Result<T, anyhow::Error>`

#### Scenario: Queue playback module uses RmpdError
- **WHEN** `rmpd-protocol/src/queue_playback.rs` functions return errors
- **THEN** they SHALL return `rmpd_core::error::Result<T>` instead of `anyhow::Result<T>`

### Requirement: anyhow restricted to binary entry point
The `anyhow` crate SHALL only be used in the `rmpd` binary crate (`rmpd/src/main.rs`), where `RmpdError` is converted to `anyhow::Error` at the process boundary.

#### Scenario: main.rs converts at boundary
- **WHEN** `main()` calls library/protocol functions that return `RmpdError`
- **THEN** it SHALL use `anyhow::Error::from()` or the `?` operator with `anyhow::Result` at the top-level only

#### Scenario: app.rs uses RmpdError
- **WHEN** `rmpd/src/app.rs` functions return errors
- **THEN** they SHALL return `rmpd_core::error::Result<T>`, with conversion to `anyhow` happening only in `main()`

### Requirement: Feature-gated From impls for library errors
The `rmpd-core/src/error.rs` SHALL provide feature-gated `From` implementations for common library error types, following the existing pattern established by `database-errors` and `player-errors` features.

#### Scenario: lofty error conversion
- **WHEN** the `library-errors` feature is enabled
- **THEN** `RmpdError` SHALL implement `From<lofty::error::LoftyError>` converting to `RmpdError::Library`

#### Scenario: tantivy error conversion
- **WHEN** the `library-errors` feature is enabled
- **THEN** `RmpdError` SHALL implement `From<tantivy::TantivyError>` converting to `RmpdError::Library`

#### Scenario: notify error conversion
- **WHEN** the `library-errors` feature is enabled
- **THEN** `RmpdError` SHALL implement `From<notify::Error>` converting to `RmpdError::Library`

### Requirement: Reduced map_err usage
Crates that enable the appropriate error feature flags SHALL use the `?` operator with automatic `From` conversion instead of explicit `.map_err()` calls where the conversion is a straightforward type mapping.

#### Scenario: Database query error propagation
- **WHEN** `rmpd-library/src/database.rs` encounters a `rusqlite::Error`
- **THEN** it SHALL propagate with `?` using the `From<rusqlite::Error>` impl instead of `.map_err(|e| RmpdError::Database(e.to_string()))`

#### Scenario: Context-specific error messages preserved
- **WHEN** a `.map_err()` call adds context beyond what the `From` impl provides (e.g., including a file path or operation name)
- **THEN** it SHALL be kept as an explicit `.map_err()` since the additional context is valuable

### Requirement: Protocol error feature flag
The `rmpd-core` crate SHALL provide a `protocol-errors` feature flag gating error conversions relevant to the protocol crate (e.g., mdns-sd errors for discovery).

#### Scenario: mdns-sd error conversion
- **WHEN** the `protocol-errors` feature is enabled
- **THEN** `RmpdError` SHALL implement `From<mdns_sd::Error>` converting to `RmpdError::Protocol`
