use crate::enums::{EMsg, EResult};

/// Errors returned by this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A socket or file error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The WebSocket transport failed.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    /// An HTTP request (Web API, CDN) failed.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// A protobuf body didn't decode.
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    /// A packet or header is malformed.
    #[error("malformed packet: {0}")]
    Malformed(&'static str),
    /// A reply had an unexpected message type.
    #[error("unexpected message {0:?}")]
    UnexpectedMessage(EMsg),
    /// Steam answered with an error result.
    #[error("Steam returned {0:?}")]
    Steam(EResult),
    /// Steam answered with an error result and a message.
    #[error("Steam returned {result:?}: {message}")]
    SteamMessage {
        /// The result.
        result: EResult,
        /// Steam's message.
        message: String,
    },
    /// Encryption or decryption failed.
    #[error("crypto error: {0}")]
    Crypto(String),
    /// Data (a manifest, chunk, KeyValues, …) is invalid.
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// An HTTP request returned a non-success status.
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    /// The client isn't connected.
    #[error("not connected")]
    NotConnected,
    /// Steam failed the job (`DestJobFailed`).
    #[error("the job failed on the server (DestJobFailed)")]
    JobFailed,
    /// No answer in time.
    #[error("timed out")]
    Timeout,
    /// Any other error.
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
