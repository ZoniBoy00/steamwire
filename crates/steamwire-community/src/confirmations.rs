//! Mobile confirmations: what the Steam mobile app's confirmation page
//! lists, such as trade offers and market listings, accepted or cancelled
//! with keys made from the authenticator's `identity_secret`.
//!
//! The requests follow SteamHelper-rs's `steam-mobile` (MIT, Copyright (c)
//! 2020 Martin Mariano).

use serde::Deserialize;
use steamwire::enums::EMobileConfirmationType;
use steamwire::handlers::auth::guard::{self, Secret};

use crate::error::{Error, Result};
use crate::session::{json, WebSession};

/// A pending confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Confirmation {
    /// Its ID (`cid`).
    pub id: u64,
    /// Its key (`ck`, the `nonce`), sent back to answer it.
    pub key: u64,
    /// What it confirms.
    pub kind: EMobileConfirmationType,
    /// The kind as Steam names it, such as "Trade Offer".
    pub type_name: String,
    /// The object it confirms: the trade offer's or the listing's ID.
    pub creator_id: u64,
    /// When it was created (Unix time).
    pub creation_time: u64,
    /// The headline the app shows, such as the trade partner's name.
    pub headline: String,
    /// The app's lines of detail.
    pub summary: Vec<String>,
}

impl Confirmation {
    /// The trade offer it confirms, if it confirms one.
    pub fn trade_offer_id(&self) -> Option<u64> {
        (self.kind == EMobileConfirmationType::Trade).then_some(self.creator_id)
    }
}

#[derive(Deserialize)]
struct ListResponse {
    success: bool,
    #[serde(default)]
    needauth: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    conf: Vec<RawConfirmation>,
}

#[derive(Deserialize)]
struct RawConfirmation {
    #[serde(rename = "type")]
    kind: i32,
    #[serde(default)]
    type_name: String,
    id: String,
    creator_id: String,
    nonce: String,
    #[serde(default)]
    creation_time: u64,
    #[serde(default)]
    headline: String,
    #[serde(default)]
    summary: Vec<String>,
}

impl TryFrom<RawConfirmation> for Confirmation {
    type Error = Error;

    fn try_from(raw: RawConfirmation) -> Result<Self> {
        let number = |text: &str, what: &str| {
            text.parse::<u64>().map_err(|_| Error::invalid(format!("confirmation {what} is not a number: {text:?}")))
        };
        Ok(Self {
            id: number(&raw.id, "id")?,
            key: number(&raw.nonce, "nonce")?,
            kind: EMobileConfirmationType(raw.kind),
            type_name: raw.type_name,
            creator_id: number(&raw.creator_id, "creator_id")?,
            creation_time: raw.creation_time,
            headline: raw.headline,
            summary: raw.summary,
        })
    }
}

#[derive(Deserialize)]
struct SuccessResponse {
    success: bool,
    #[serde(default)]
    message: Option<String>,
}

/// The confirmations of the account a [`WebSession`] is logged in to.
///
/// ```no_run
/// # async fn run(session: steamwire_community::WebSession, identity_secret: &str) -> steamwire_community::Result<()> {
/// use steamwire::handlers::auth::guard::Secret;
/// use steamwire_community::Confirmations;
///
/// let confirmations = Confirmations::new(session, Secret::from_base64(identity_secret)?);
/// for confirmation in confirmations.list().await? {
///     if confirmation.trade_offer_id().is_some() {
///         confirmations.accept(&[confirmation]).await?;
///     }
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Confirmations {
    session: WebSession,
    identity_secret: Secret,
    device_id: String,
    time_offset: i64,
}

impl Confirmations {
    /// Uses the authenticator's `identity_secret`, and the device ID the
    /// mobile app derives from the Steam ID.
    pub fn new(session: WebSession, identity_secret: Secret) -> Self {
        let device_id = guard::device_id(&session.steam_id().as_u64().to_string());
        Self { session, identity_secret, device_id, time_offset: 0 }
    }

    /// Uses another device ID, such as an maFile's `device_id`.
    pub fn with_device_id(mut self, device_id: impl Into<String>) -> Self {
        self.device_id = device_id.into();
        self
    }

    /// Seconds Steam's clock is ahead of ours
    /// ([`guard::server_time_offset`]); the keys are only valid for the
    /// time they were made for.
    pub fn with_time_offset(mut self, offset: i64) -> Self {
        self.time_offset = offset;
        self
    }

    fn params(&self, tag: &str) -> Vec<(&'static str, String)> {
        let time = guard::now().saturating_add_signed(self.time_offset);
        vec![
            ("p", self.device_id.clone()),
            ("a", self.session.steam_id().as_u64().to_string()),
            ("k", guard::generate_confirmation_key(&self.identity_secret, time, tag)),
            ("t", time.to_string()),
            ("m", "react".into()),
            ("tag", tag.into()),
        ]
    }

    /// The pending confirmations
    /// (`GET https://steamcommunity.com/mobileconf/getlist`).
    pub async fn list(&self) -> Result<Vec<Confirmation>> {
        let url = format!("{}/mobileconf/getlist", self.session.urls().community);
        let response: ListResponse = json(self.session.get(&url, &self.params("conf")).await?).await?;
        if !response.success {
            if response.needauth {
                return Err(Error::SessionExpired);
            }
            return Err(Error::Steam { result: steamwire::EResult::Fail, message: response.message });
        }
        response.conf.into_iter().map(Confirmation::try_from).collect()
    }

    /// Accepts `confirmations`.
    pub async fn accept(&self, confirmations: &[Confirmation]) -> Result<()> {
        self.respond(confirmations, true).await
    }

    /// Cancels (denies) `confirmations`.
    pub async fn cancel(&self, confirmations: &[Confirmation]) -> Result<()> {
        self.respond(confirmations, false).await
    }

    /// Accepts or cancels `confirmations` in one request
    /// (`POST https://steamcommunity.com/mobileconf/multiajaxop`).
    pub async fn respond(&self, confirmations: &[Confirmation], accept: bool) -> Result<()> {
        if confirmations.is_empty() {
            return Ok(());
        }
        let mut form = self.params("conf");
        form.push(("op", if accept { "allow" } else { "cancel" }.into()));
        for confirmation in confirmations {
            form.push(("cid[]", confirmation.id.to_string()));
            form.push(("ck[]", confirmation.key.to_string()));
        }
        let url = format!("{}/mobileconf/multiajaxop", self.session.urls().community);
        let response: SuccessResponse = json(self.session.post(&url, &form, None).await?).await?;
        if response.success {
            Ok(())
        } else {
            Err(Error::Steam { result: steamwire::EResult::Fail, message: response.message })
        }
    }

    /// Accepts the confirmation of the trade offer `offer_id`, if there is
    /// one; returns whether there was.
    pub async fn accept_trade_offer(&self, offer_id: u64) -> Result<bool> {
        let found: Vec<_> = self.list().await?.into_iter().filter(|c| c.trade_offer_id() == Some(offer_id)).collect();
        if found.is_empty() {
            return Ok(false);
        }
        self.accept(&found).await?;
        Ok(true)
    }
}
