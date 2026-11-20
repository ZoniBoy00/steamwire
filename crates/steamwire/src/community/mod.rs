//! Steam's web side (the `community` feature): what goes through steamcommunity.com,
//! the store and the Web API instead of the client protocol.
//!
//! - [`WebSession`]: the login cookies and access token, made from a
//!   logged-on [`crate::SteamClient`] and renewed through it.
//! - [`TradeOffers`], [`TradeLink`] and [`Inventories`]: listing, sending,
//!   accepting, declining and cancelling trade offers, trade holds and
//!   inventories.
//! - [`Confirmations`]: the mobile confirmations of trades and market
//!   listings, with the authenticator's `identity_secret`.
//! - [`authenticator`]: adding (and finalizing) and removing the Steam Guard
//!   mobile authenticator, with Steam Desktop Authenticator's `.maFile`.
//! - [`Store`]: the cart and buying it with the Steam Wallet.
//!
//! The web session, confirmations, trade offers and inventories are ported
//! from SteamHelper-rs (MIT); see the README's References.
//!
//! ```no_run
//! # async fn run(client: steamwire::SteamClient, refresh_token: String) -> steamwire::community::Result<()> {
//! use steamwire::community::{OfferFilter, TradeOffers, WebSession};
//!
//! let session = WebSession::from_client(&client, refresh_token).await?;
//! let offers = TradeOffers::new(session).list(OfferFilter::default()).await?;
//! for offer in &offers.received {
//!     println!("offer {} from {}: {:?}", offer.id, offer.partner.steam3(), offer.state);
//! }
//! # Ok(()) }
//! ```

pub mod authenticator;
mod confirmations;
mod error;
mod inventory;
mod session;
mod store;
mod trade;
mod util;

pub use authenticator::MobileAuth;
pub use confirmations::{Confirmation, Confirmations};
pub use error::{Error, Result};
pub use inventory::{Asset, Description, Inventories, Item};
pub use session::{AccessToken, ClientTokenIssuer, TokenIssuer, Urls, WebSession, WebSessionBuilder};
pub use store::{Amount, Cart, CartEntry, CartItem, PurchaseOutcome, Quote, Receipt, Store};
pub use trade::{
    NewOffer, OfferFilter, OfferItem, OfferList, OfferOutcome, TradeHold, TradeLink, TradeOffer, TradeOffers,
};
