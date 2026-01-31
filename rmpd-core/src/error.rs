use thiserror::Error;

#[derive(Error, Debug)]
pub enum RmpdError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Player error: {0}")]
    Player(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Library error: {0}")]
    Library(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Permission denied")]
    PermissionDenied,
}

pub type Result<T> = std::result::Result<T, RmpdError>;

// Automatic error conversions for common dependency errors
#[cfg(feature = "database-errors")]
impl From<rusqlite::Error> for RmpdError {
    fn from(err: rusqlite::Error) -> Self {
        RmpdError::Database(err.to_string())
    }
}

#[cfg(feature = "player-errors")]
impl From<symphonia::core::errors::Error> for RmpdError {
    fn from(err: symphonia::core::errors::Error) -> Self {
        RmpdError::Player(err.to_string())
    }
}

#[cfg(feature = "player-errors")]
impl From<cpal::BuildStreamError> for RmpdError {
    fn from(err: cpal::BuildStreamError) -> Self {
        RmpdError::Player(format!("Failed to build stream: {}", err))
    }
}

#[cfg(feature = "player-errors")]
impl From<cpal::PlayStreamError> for RmpdError {
    fn from(err: cpal::PlayStreamError) -> Self {
        RmpdError::Player(format!("Stream playback error: {}", err))
    }
}
