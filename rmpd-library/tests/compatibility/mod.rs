/// Compatibility test module
///
/// Tests that validate rmpd behaves identically to MPD for:
/// - Metadata extraction
/// - Database queries
/// - Search operations
/// - Artwork handling

mod metadata_tests;
mod database_tests;
mod search_tests;
mod edge_cases_tests;
mod artwork_tests;
