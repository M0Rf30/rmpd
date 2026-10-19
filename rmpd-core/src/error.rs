// SPDX-FileCopyrightText: 2026 Gianluca Boiano
// SPDX-License-Identifier: MIT OR Apache-2.0

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

    /// A demuxer/decoder failure, with the kind of the underlying media error
    /// preserved. `Display` matches [`RmpdError::Player`] so protocol `ACK`
    /// text is the same either way; use [`RmpdError::player_message`] for the
    /// bare text.
    #[error("Player error: {message}")]
    Media {
        kind: MediaErrorKind,
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

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
}

/// What went wrong in a demuxer or decoder (mirrors `symphonia`'s error classes
/// without making that crate a hard dependency of `rmpd-core`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaErrorKind {
    /// Reading the source failed; carries the I/O error kind
    /// (`UnexpectedEof` marks a truncated stream).
    Io(std::io::ErrorKind),
    /// Malformed data; the packet or frame can be skipped.
    Decode,
    /// The source could not be seeked.
    Seek,
    /// An unsupported container or codec feature.
    Unsupported,
    /// A decoder safety limit was reached.
    Limit,
    /// The demuxer or decoder must be rebuilt before continuing.
    ResetRequired,
    /// Any error class this enum does not know yet.
    Other,
}

impl RmpdError {
    /// The error text without the `"Player error: "`-style category prefix
    /// `Display` adds, for the variants that describe a playback failure
    /// ([`RmpdError::Player`], [`RmpdError::Media`]).
    #[must_use]
    pub fn player_message(&self) -> Option<&str> {
        match self {
            Self::Player(msg) | Self::Media { message: msg, .. } => Some(msg),
            _ => None,
        }
    }

    /// The media error kind, if this error came from a demuxer or decoder.
    #[must_use]
    pub fn media_kind(&self) -> Option<MediaErrorKind> {
        match self {
            Self::Media { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// `true` for a truncated source: reading hit an unexpected end of file.
    #[must_use]
    pub fn is_unexpected_eof(&self) -> bool {
        self.media_kind() == Some(MediaErrorKind::Io(std::io::ErrorKind::UnexpectedEof))
    }
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
        let message = err.to_string();
        Self::from_symphonia_message(err, message)
    }
}

#[cfg(feature = "player-errors")]
impl RmpdError {
    /// Wrap a symphonia error, prefixing its text with `context`
    /// (`"<context>: <error>"`) while keeping its kind and cause.
    #[must_use]
    pub fn from_symphonia_context(context: &str, err: symphonia::core::errors::Error) -> Self {
        let message = format!("{context}: {err}");
        Self::from_symphonia_message(err, message)
    }

    fn from_symphonia_message(err: symphonia::core::errors::Error, message: String) -> Self {
        use symphonia::core::errors::Error as E;
        let kind = match &err {
            E::IoError(e) => MediaErrorKind::Io(e.kind()),
            E::DecodeError(_) => MediaErrorKind::Decode,
            E::SeekError(_) => MediaErrorKind::Seek,
            E::Unsupported(_) => MediaErrorKind::Unsupported,
            E::LimitError(_) => MediaErrorKind::Limit,
            E::ResetRequired => MediaErrorKind::ResetRequired,
            _ => MediaErrorKind::Other,
        };
        Self::Media {
            kind,
            message,
            source: Some(Box::new(err)),
        }
    }
}

#[cfg(feature = "player-errors")]
impl From<cpal::Error> for RmpdError {
    fn from(err: cpal::Error) -> Self {
        RmpdError::Player(format!("CPAL error: {err}"))
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

#[cfg(all(test, feature = "player-errors"))]
mod tests {
    use super::*;
    use symphonia::core::errors::{Error as SymError, SeekErrorKind};

    #[test]
    fn symphonia_error_kind_is_preserved() {
        let cases = [
            (SymError::DecodeError("bad frame"), MediaErrorKind::Decode),
            (
                SymError::SeekError(SeekErrorKind::Unseekable),
                MediaErrorKind::Seek,
            ),
            (SymError::Unsupported("codec"), MediaErrorKind::Unsupported),
            (SymError::LimitError("too big"), MediaErrorKind::Limit),
            (SymError::ResetRequired, MediaErrorKind::ResetRequired),
            (
                SymError::IoError(std::io::ErrorKind::UnexpectedEof.into()),
                MediaErrorKind::Io(std::io::ErrorKind::UnexpectedEof),
            ),
        ];
        for (err, kind) in cases {
            let text = err.to_string();
            let converted = RmpdError::from(err);
            assert_eq!(converted.media_kind(), Some(kind), "{text}");
            // The cause survives for `Error::source` walkers.
            assert!(std::error::Error::source(&converted).is_some(), "{text}");
        }
    }

    #[test]
    fn display_stays_compatible_with_the_player_variant() {
        let sym = SymError::DecodeError("bad frame");
        let legacy = RmpdError::Player(sym.to_string()).to_string();
        let converted = RmpdError::from(sym);
        assert_eq!(converted.to_string(), legacy);
        assert_eq!(
            converted.to_string(),
            "Player error: malformed stream: bad frame"
        );
        assert_eq!(
            converted.player_message(),
            Some("malformed stream: bad frame")
        );
    }

    #[test]
    fn context_prefixes_the_message_and_keeps_the_kind() {
        let e = RmpdError::from_symphonia_context(
            "Failed to decode packet",
            SymError::DecodeError("bad frame"),
        );
        assert_eq!(
            e.player_message(),
            Some("Failed to decode packet: malformed stream: bad frame")
        );
        assert_eq!(e.media_kind(), Some(MediaErrorKind::Decode));
        assert!(!e.is_unexpected_eof());
    }

    #[test]
    fn unexpected_eof_is_detected_by_kind_not_text() {
        let eof = RmpdError::from_symphonia_context(
            "Failed to decode packet",
            SymError::IoError(std::io::ErrorKind::UnexpectedEof.into()),
        );
        assert!(eof.is_unexpected_eof());
        let other_io = RmpdError::from(SymError::IoError(std::io::ErrorKind::NotFound.into()));
        assert!(!other_io.is_unexpected_eof());
        assert!(!RmpdError::Player("end of stream".to_owned()).is_unexpected_eof());
    }

    #[test]
    fn plain_player_error_has_no_media_kind() {
        let e = RmpdError::Player("Not seekable".to_owned());
        assert_eq!(e.media_kind(), None);
        assert_eq!(e.player_message(), Some("Not seekable"));
        assert_eq!(RmpdError::NotFound("x".into()).player_message(), None);
    }
}
