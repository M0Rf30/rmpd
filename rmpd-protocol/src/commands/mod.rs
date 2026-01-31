//! MPD protocol command handlers organized by category
//!
//! This module splits the large server.rs file into logical categories for better
//! organization and maintainability.

pub mod connection;
pub mod database;
pub mod fingerprint;
pub mod messaging;
pub mod options;
pub mod outputs;
pub mod partition;
pub mod playback;
pub mod playlists;
pub mod queue;
pub mod reflection;
pub mod stickers;
pub mod storage;

// Re-export commonly used types
pub use crate::response::{Response, ResponseBuilder, Stats};
pub use crate::state::AppState;
