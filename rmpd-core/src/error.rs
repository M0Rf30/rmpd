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
