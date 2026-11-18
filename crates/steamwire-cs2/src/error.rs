use steamwire::SteamId;

use crate::{InspectLinkError, ItemDetailsError};

/// What went wrong in a [`Cs2`](crate::Cs2) session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Sending to Steam failed.
    #[error(transparent)]
    Client(#[from] steamwire::Error),
    /// [`Cs2::connect`](crate::Cs2::connect) needs a logged-on client.
    #[error("the client isn't logged on")]
    NotLoggedOn,
    /// The client disconnected: the session can't go on. Connect again with a
    /// new client.
    #[error("the client disconnected")]
    Closed,
    /// The game coordinator didn't answer in time: the message type waited for.
    /// It doesn't answer about items it can't find, nor requests that come too
    /// fast.
    #[error("CS2's game coordinator didn't answer (message {0}) in time")]
    Timeout(u32),
    /// The game coordinator refused the session (`ClientLogonFatalError`).
    #[error("CS2's game coordinator refused the session ({code}): {message}")]
    Refused {
        /// Its error code.
        code: u32,
        /// Its message.
        message: String,
    },
    /// A request about a player needs an individual account in the public
    /// universe.
    #[error("{0} isn't an individual account")]
    NotIndividual(SteamId),
    /// The string isn't an inspect link.
    #[error(transparent)]
    InspectLink(#[from] InspectLinkError),
    /// The answer has no item, or one without a def index.
    #[error(transparent)]
    ItemDetails(#[from] ItemDetailsError),
    /// A message of the game coordinator couldn't be decoded.
    #[error("undecodable message from CS2's game coordinator: {0}")]
    Decode(#[from] prost::DecodeError),
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
