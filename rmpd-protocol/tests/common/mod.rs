//! Common test utilities for integration tests
//!
//! These utilities provide helpers for testing MPD protocol response formats.
//! Currently focused on response parsing validation rather than live server tests.

pub mod state_helpers;

/// Test helper for MPD protocol response validation
pub struct TestClient;

impl TestClient {
    /// Check if response is OK
    pub fn is_ok(response: &str) -> bool {
        response.trim().ends_with("OK")
    }

    /// Check if response is an error
    pub fn is_error(response: &str) -> bool {
        response.starts_with("ACK")
    }

    /// Extract field value from response
    pub fn get_field<'a>(response: &'a str, field: &str) -> Option<&'a str> {
        let prefix = format!("{}: ", field);
        response
            .lines()
            .find(|line| line.starts_with(&prefix))
            .map(|line| line.trim_start_matches(&prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ok() {
        assert!(TestClient::is_ok("OK\n"));
        assert!(TestClient::is_ok("field: value\nOK\n"));
        assert!(!TestClient::is_ok("ACK [5@0] {command} error\n"));
    }

    #[test]
    fn test_is_error() {
        assert!(TestClient::is_error("ACK [5@0] {command} error\n"));
        assert!(!TestClient::is_error("OK\n"));
    }

    #[test]
    fn test_get_field() {
        let response = "volume: 100\nrepeat: 0\nOK\n";
        assert_eq!(TestClient::get_field(response, "volume"), Some("100"));
        assert_eq!(TestClient::get_field(response, "repeat"), Some("0"));
        assert_eq!(TestClient::get_field(response, "missing"), None);
    }
}
