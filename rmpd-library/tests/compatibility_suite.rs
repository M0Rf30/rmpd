/// Compatibility test suite entry point
///
/// This test file runs all compatibility tests that validate rmpd's
/// behavior matches MPD for metadata extraction and database operations.

mod common;
mod fixtures;
mod compatibility;

// Re-export test modules so they run
// The actual tests are in the submodules
