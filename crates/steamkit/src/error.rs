use crate::enums::{EMsg, EResult};

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("malformed packet: {0}")]
    Malformed(&'static str),
    #[error("unexpected message {0:?}")]
    UnexpectedMessage(EMsg),
    #[error("Steam returned {0:?}")]
    Steam(EResult),
    #[error("Steam returned {result:?}: {message}")]
    SteamMessage { result: EResult, message: String },
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("invalid data: {0}")]
    InvalidData(String),
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    #[error("not connected")]
    NotConnected,
    #[error("the job failed on the server (DestJobFailed)")]
    JobFailed,
    #[error("timed out")]
    Timeout,
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// The result Steam returned, for [`Error::Steam`] and
    /// [`Error::SteamMessage`].
    pub fn result(&self) -> Option<EResult> {
        match self {
            Error::Steam(result) | Error::SteamMessage { result, .. } => Some(*result),
            _ => None,
        }
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(Box::new(e))
    }
}

/// Result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
