use steamwire::EResult;

/// Errors of the web side.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A call over the Steam client connection failed.
    #[error(transparent)]
    Client(#[from] steamwire::Error),
    /// The HTTP request failed.
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    /// Steam answered with an HTTP error status.
    #[error("HTTP status {0}")]
    HttpStatus(u16),
    /// Steam redirected to its login page or rejected the session's cookies:
    /// the access token was revoked or has expired.
    #[error("the web session is no longer logged in")]
    SessionExpired,
    /// Steam reported a failure, with its message when it gave one.
    #[error("Steam: {result:?}{}", message.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Steam {
        /// The result (`Fail` when Steam gave only a message).
        result: EResult,
        /// Steam's message, if any.
        message: Option<String>,
    },
    /// The answer was not what Steam sends.
    #[error("unexpected response: {0}")]
    InvalidResponse(String),
    /// An argument can't be used, such as a malformed trade link.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The activation code sent by SMS or email was wrong.
    #[error("the activation code is incorrect")]
    ActivationCodeMismatch,
    /// Steam kept rejecting the authenticator codes while finalizing: the
    /// clock is probably wrong.
    #[error("Steam did not accept the authenticator's codes")]
    CodesRejected,
    /// The revocation code was wrong.
    #[error("incorrect revocation code ({attempts_remaining} attempts remaining)")]
    IncorrectRevocationCode {
        /// Wrong codes Steam still accepts before it locks the account.
        attempts_remaining: u32,
    },
    /// A purchase was stopped before anything was charged; the reason says
    /// why.
    #[error("purchase refused: {0}")]
    PurchaseRefused(String),
    /// The purchase was sent, but how it ended could not be read. Check the
    /// account's purchase history before trying again.
    #[error("the purchase was sent but its outcome is unknown: {0}")]
    PurchaseUnconfirmed(String),
    /// Reading or writing a file failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The `EResult` behind the error, if there is one.
    pub fn result(&self) -> Option<EResult> {
        match self {
            Self::Client(e) => e.result(),
            Self::Steam { result, .. } => Some(*result),
            _ => None,
        }
    }

    pub(crate) fn steam(result: EResult) -> Self {
        Self::Steam { result, message: None }
    }

    pub(crate) fn invalid(what: impl std::fmt::Display) -> Self {
        Self::InvalidResponse(what.to_string())
    }
}

/// The result type of this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
