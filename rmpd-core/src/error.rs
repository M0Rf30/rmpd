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

    #[error("Storage error: {0}")]
    Storage(String),

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
impl From<cpal::Error> for RmpdError {
    fn from(err: cpal::Error) -> Self {
        RmpdError::Player(format!("CPAL error: {err}"))
    }
}

#[cfg(feature = "library-errors")]
impl From<lofty::error::LoftyError> for RmpdError {
    fn from(err: lofty::error::LoftyError) -> Self {
        RmpdError::Library(err.to_string())
    }
}

#[cfg(feature = "library-errors")]
impl From<tantivy::TantivyError> for RmpdError {
    fn from(err: tantivy::TantivyError) -> Self {
        RmpdError::Library(err.to_string())
    }
}

#[cfg(feature = "library-errors")]
impl From<notify::Error> for RmpdError {
    fn from(err: notify::Error) -> Self {
        RmpdError::Library(err.to_string())
    }
}

#[cfg(feature = "protocol-errors")]
impl From<mdns_sd::Error> for RmpdError {
    fn from(err: mdns_sd::Error) -> Self {
        RmpdError::Protocol(err.to_string())
    }
}
