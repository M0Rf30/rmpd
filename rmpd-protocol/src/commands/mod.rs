//! MPD protocol command handlers organized by category
//!
//! This module splits the large server.rs file into logical categories for better
//! organization and maintainability.

pub mod database;
pub mod options;
pub mod playback;
pub mod queue;

// TODO: Extract remaining command modules
// pub mod connection;
// pub mod outputs;
// pub mod playlists;
// pub mod reflection;
// pub mod status;
// pub mod stickers;

// Re-export commonly used types
pub use crate::response::{Response, ResponseBuilder, Stats};
pub use crate::state::AppState;
