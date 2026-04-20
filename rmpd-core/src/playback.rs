//! Playback-related types and utilities

use crate::song::Song;
use camino::Utf8PathBuf;
use std::sync::Arc;

/// A song prepared for playback with a resolved filesystem path.
/// Avoids cloning the full Song — shares it via Arc.
pub struct PlaybackSong {
    pub song: Arc<Song>,
    pub resolved_path: Utf8PathBuf,
}
