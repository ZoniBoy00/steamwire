//! Trade offers: listing them (`IEconService`), sending, accepting,
//! declining and cancelling them (steamcommunity.com), trade links and
//! trade holds.
//!
//! The requests follow SteamHelper-rs's `steam-trading` (MIT, Copyright (c)
//! 2020 Martin Mariano), checked against node-steam-tradeoffer-manager.

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use crate::enums::ETradeOfferState;
use crate::SteamId;
use serde::{Deserialize, Serialize};

use crate::community::error::{Error, Result};
use crate::community::inventory::Asset;
use crate::community::session::{json, WebSession};
use crate::community::util::{flexible_u64, optional_flexible_u64};

/// At most this many `GetTradeOffers` pages are read in one call, in case
/// Steam's cursor never ends.
const MAX_OFFER_PAGES: usize = 100;

/// A trade link: `https://steamcommunity.com/tradeoffer/new/?partner=…&token=…`.
/// Offers to someone who isn't a friend need its token.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TradeLink {
    /// Whose link it is.
    pub partner: SteamId,
    /// The access token; friends can be sent offers without one.
    pub token: Option<String>,
}

impl fmt::Debug for TradeLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url())
    }
}

impl TradeLink {
    /// A link to `partner` with `token`.
    pub fn new(partner: SteamId, token: Option<String>) -> Self {
        Self { partner, token }
    }

    /// Reads a trade link, including one HTML-escaped (`&amp;token=`).
    pub fn parse(link: &str) -> Result<Self> {
        let invalid = || Error::InvalidInput(format!("not a trade link: {link}"));
        let (_, query) = link.split_once("tradeoffer/new/?").ok_or_else(invalid)?;
        // The link ends where an attribute or the text around it does.
        let end = query.find(|c: char| "\"'<>".contains(c) || c.is_whitespace()).unwrap_or(query.len());
        let query = query[..end].replace("&amp;", "&");
        let mut partner = None;
        let mut token = None;
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("partner=") {
                partner = value.parse::<u32>().ok();
            } else if let Some(value) = pair.strip_prefix("token=") {
                let valid = !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
                token = valid.then(|| value.to_string());
            }
        }
        let partner = partner.filter(|&p| p != 0).ok_or_else(invalid)?;
        Ok(Self { partner: SteamId::from_account_id(partner), token })
    }

    /// The link.
    pub fn url(&self) -> String {
        let mut url = format!("https://steamcommunity.com/tradeoffer/new/?partner={}", self.partner.account_id());
        if let Some(token) = &self.token {
            url.push_str("&token=");
            url.push_str(token);
        }
        url
    }
}

/// An item of a trade offer, as `IEconService` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct OfferItem {
    /// The app the item belongs to.
    pub appid: u32,
    /// Its inventory context.
    #[serde(deserialize_with = "flexible_u64")]
    pub contextid: u64,
    /// Its asset ID.
    #[serde(deserialize_with = "flexible_u64")]
    pub assetid: u64,
    /// Its class.
    #[serde(default, deserialize_with = "flexible_u64")]
    pub classid: u64,
    /// Its instance.
    #[serde(default, deserialize_with = "flexible_u64")]
    pub instanceid: u64,
    /// How many, for stackable items.
    #[serde(default = "one", deserialize_with = "flexible_u64")]
    pub amount: u64,
    /// The item is no longer in the inventory.
    #[serde(default)]
    pub missing: bool,
}

fn one() -> u64 {
    1
}

impl OfferItem {
    /// The asset, to use in a new offer.
    pub fn asset(&self) -> Asset {
        Asset { appid: self.appid, contextid: self.contextid, assetid: self.assetid, amount: self.amount }
    }
}

#[derive(Deserialize)]
struct RawOffer {
    #[serde(deserialize_with = "flexible_u64")]
    tradeofferid: u64,
    accountid_other: u32,
    #[serde(default)]
    message: String,
    #[serde(default)]
    expiration_time: u64,
    trade_offer_state: i32,
    #[serde(default)]
    items_to_give: Vec<OfferItem>,
    #[serde(default)]
    items_to_receive: Vec<OfferItem>,
    #[serde(default)]
    is_our_offer: bool,
    #[serde(default)]
    time_created: u64,
    #[serde(default)]
    time_updated: u64,
    #[serde(default, deserialize_with = "optional_flexible_u64")]
    tradeid: Option<u64>,
    #[serde(default)]
    from_real_time_trade: bool,
    #[serde(default)]
    escrow_end_date: u64,
    #[serde(default)]
    confirmation_method: i32,
}

/// A trade offer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TradeOffer {
    /// Its ID.
    pub id: u64,
    /// The other side.
    pub partner: SteamId,
    /// The message sent with it.
    pub message: String,
    /// Where it stands.
    pub state: ETradeOfferState,
    /// What this account gives.
    pub items_to_give: Vec<OfferItem>,
    /// What this account gets.
    pub items_to_receive: Vec<OfferItem>,
    /// This account sent it.
    pub is_our_offer: bool,
    /// When it was made (Unix time).
    pub time_created: u64,
    /// When it last changed.
    pub time_updated: u64,
    /// When it expires.
    pub expiration_time: u64,
    /// When the items are delivered, for an offer on hold (0 otherwise).
    pub escrow_end_date: u64,
    /// The trade it became, once accepted.
    pub trade_id: Option<u64>,
    /// It came from a real-time trade.
    pub from_real_time_trade: bool,
    /// How it waits for confirmation (`ETradeOfferConfirmationMethod`:
    /// 1 email, 2 mobile).
    pub confirmation_method: i32,
}

impl From<RawOffer> for TradeOffer {
    fn from(raw: RawOffer) -> Self {
        Self {
            id: raw.tradeofferid,
            partner: SteamId::from_account_id(raw.accountid_other),
            message: raw.message,
            state: ETradeOfferState(raw.trade_offer_state),
            items_to_give: raw.items_to_give,
            items_to_receive: raw.items_to_receive,
            is_our_offer: raw.is_our_offer,
            time_created: raw.time_created,
            time_updated: raw.time_updated,
            expiration_time: raw.expiration_time,
            escrow_end_date: raw.escrow_end_date,
            trade_id: raw.tradeid.filter(|&id| id != 0),
            from_real_time_trade: raw.from_real_time_trade,
            confirmation_method: raw.confirmation_method,
        }
    }
}

/// Which offers [`TradeOffers::list`] reads.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OfferFilter {
    /// Offers this account sent.
    pub sent: bool,
    /// Offers this account received.
    pub received: bool,
    /// Only offers still active (and those that changed recently).
    pub active_only: bool,
}

impl Default for OfferFilter {
    /// Active offers, both ways.
    fn default() -> Self {
        Self { sent: true, received: true, active_only: true }
    }
}

/// Offers [`TradeOffers::list`] found.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct OfferList {
    /// Sent by this account.
    pub sent: Vec<TradeOffer>,
    /// Received by this account.
    pub received: Vec<TradeOffer>,
}

#[derive(Deserialize, Default)]
struct OffersPage {
    #[serde(default)]
    trade_offers_sent: Vec<RawOffer>,
    #[serde(default)]
    trade_offers_received: Vec<RawOffer>,
    #[serde(default)]
    next_cursor: Option<u32>,
}

#[derive(Deserialize)]
struct ApiResponse<T> {
    response: T,
}

#[derive(Deserialize)]
struct OfferResponse {
    offer: Option<RawOffer>,
}

#[derive(Deserialize)]
struct HoldResponse {
    #[serde(default)]
    my_escrow: Option<Escrow>,
    #[serde(default)]
    their_escrow: Option<Escrow>,
}

#[derive(Deserialize)]
struct Escrow {
    #[serde(deserialize_with = "flexible_u64")]
    escrow_end_duration_seconds: u64,
}

/// How long a trade with someone would be held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TradeHold {
    /// This account's side.
    pub mine: Duration,
    /// The partner's side.
    pub theirs: Duration,
}

/// A new offer: what to give and take, and to whom.
#[derive(Clone, Debug)]
pub struct NewOffer {
    to: TradeLink,
    give: Vec<Asset>,
    take: Vec<Asset>,
    message: String,
}

impl NewOffer {
    /// An offer to the owner of `link`.
    pub fn to(link: TradeLink) -> Self {
        Self { to: link, give: Vec::new(), take: Vec::new(), message: String::new() }
    }

    /// Gives `asset`.
    pub fn give(mut self, asset: Asset) -> Self {
        self.give.push(asset);
        self
    }

    /// Asks for `asset`.
    pub fn take(mut self, asset: Asset) -> Self {
        self.take.push(asset);
        self
    }

    /// Sends `message` with the offer.
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub(crate) fn form(&self, session_id: &str) -> Vec<(&'static str, String)> {
        #[derive(Serialize)]
        struct JsonAsset {
            appid: u32,
            contextid: String,
            amount: u64,
            assetid: String,
        }
        #[derive(Serialize)]
        struct Side {
            assets: Vec<JsonAsset>,
            currency: [(); 0],
            ready: bool,
        }
        #[derive(Serialize)]
        struct JsonOffer {
            newversion: bool,
            version: u32,
            me: Side,
            them: Side,
        }
        let side = |assets: &[Asset]| Side {
            assets: assets
                .iter()
                .map(|a| JsonAsset {
                    appid: a.appid,
                    contextid: a.contextid.to_string(),
                    amount: a.amount,
                    assetid: a.assetid.to_string(),
                })
                .collect(),
            currency: [],
            ready: false,
        };
        let offer = JsonOffer { newversion: true, version: 2, me: side(&self.give), them: side(&self.take) };
        let params = match &self.to.token {
            Some(token) => serde_json::json!({ "trade_offer_access_token": token }),
            None => serde_json::json!({}),
        };
        vec![
            ("sessionid", session_id.to_string()),
            ("serverid", "1".into()),
            ("partner", self.to.partner.as_u64().to_string()),
            ("captcha", String::new()),
            ("tradeoffermessage", self.message.clone()),
            ("json_tradeoffer", serde_json::to_string(&offer).expect("serializable")),
            ("trade_offer_create_params", params.to_string()),
        ]
    }
}

/// What Steam answered to a sent or accepted offer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct OfferOutcome {
    /// The offer's ID (sent offers).
    #[serde(default, rename = "tradeofferid", deserialize_with = "optional_flexible_u64")]
    pub offer_id: Option<u64>,
    /// The trade's ID, once Steam made the exchange (accepted offers).
    #[serde(default, rename = "tradeid", deserialize_with = "optional_flexible_u64")]
    pub trade_id: Option<u64>,
    /// It waits for a mobile confirmation ([`crate::community::Confirmations`]).
    #[serde(default)]
    pub needs_mobile_confirmation: bool,
    /// It waits for a confirmation by email.
    #[serde(default)]
    pub needs_email_confirmation: bool,
}

/// The trade offers of the account a [`WebSession`] is logged in to.
#[derive(Clone, Debug)]
pub struct TradeOffers {
    session: WebSession,
}

impl TradeOffers {
    /// Uses `session`.
    pub fn new(session: WebSession) -> Self {
        Self { session }
    }

    /// The offers `filter` selects
    /// (`IEconService/GetTradeOffers`, all pages).
    pub async fn list(&self, filter: OfferFilter) -> Result<OfferList> {
        let mut list = OfferList::default();
        let mut seen = (HashSet::new(), HashSet::new());
        let mut cursors = HashSet::new();
        let mut cursor = None;
        for _ in 0..MAX_OFFER_PAGES {
            let mut params = vec![
                ("get_sent_offers", u8::from(filter.sent).to_string()),
                ("get_received_offers", u8::from(filter.received).to_string()),
                ("get_descriptions", "0".into()),
                ("active_only", u8::from(filter.active_only).to_string()),
                ("time_historical_cutoff", u32::MAX.to_string()),
            ];
            if let Some(cursor) = cursor {
                params.push(("cursor", u32::to_string(&cursor)));
            }
            let page: ApiResponse<OffersPage> =
                self.session.api(false, "IEconService", "GetTradeOffers", 1, &params).await?;
            let page = page.response;
            for offer in page.trade_offers_sent {
                if seen.0.insert(offer.tradeofferid) {
                    list.sent.push(offer.into());
                }
            }
            for offer in page.trade_offers_received {
                if seen.1.insert(offer.tradeofferid) {
                    list.received.push(offer.into());
                }
            }
            match page.next_cursor.filter(|&c| c != 0) {
                None => return Ok(list),
                Some(next) if !cursors.insert(next) => {
                    return Err(Error::invalid(format!("GetTradeOffers repeated the cursor {next}")))
                }
                Some(next) => cursor = Some(next),
            }
        }
        Err(Error::invalid(format!("GetTradeOffers sent more than {MAX_OFFER_PAGES} pages")))
    }

    /// The offer `offer_id` (`IEconService/GetTradeOffer`).
    pub async fn get(&self, offer_id: u64) -> Result<TradeOffer> {
        let params = [("tradeofferid", offer_id.to_string())];
        let response: ApiResponse<OfferResponse> =
            self.session.api(false, "IEconService", "GetTradeOffer", 1, &params).await?;
        response.response.offer.map(TradeOffer::from).ok_or_else(|| Error::steam(crate::EResult::FileNotFound))
    }

    /// Sends `offer` (`POST https://steamcommunity.com/tradeoffer/new/send`).
    /// It usually needs a mobile confirmation next.
    pub async fn send(&self, offer: &NewOffer) -> Result<OfferOutcome> {
        let community = &self.session.urls().community;
        let url = format!("{community}/tradeoffer/new/send");
        let referer = format!("{community}/tradeoffer/new");
        let form = offer.form(self.session.session_id());
        let outcome: OfferOutcome = json(self.session.post(&url, &form, Some(&referer)).await?).await?;
        if outcome.offer_id.is_none() {
            return Err(Error::invalid("Steam sent no trade offer ID"));
        }
        Ok(outcome)
    }

    /// Accepts `offer`, one made to this account
    /// (`POST https://steamcommunity.com/tradeoffer/{id}/accept`).
    pub async fn accept(&self, offer: &TradeOffer) -> Result<OfferOutcome> {
        let community = &self.session.urls().community;
        let url = format!("{community}/tradeoffer/{}/accept", offer.id);
        let referer = format!("{community}/tradeoffer/{}/", offer.id);
        let form = [
            ("sessionid", self.session.session_id().to_string()),
            ("serverid", "1".into()),
            ("partner", offer.partner.as_u64().to_string()),
            ("captcha", String::new()),
            ("tradeofferid", offer.id.to_string()),
        ];
        json(self.session.post(&url, &form, Some(&referer)).await?).await
    }

    /// Declines `offer_id`, an offer made to this account.
    pub async fn decline(&self, offer_id: u64) -> Result<()> {
        self.close(offer_id, "decline").await
    }

    /// Cancels `offer_id`, an offer this account sent.
    pub async fn cancel(&self, offer_id: u64) -> Result<()> {
        self.close(offer_id, "cancel").await
    }

    async fn close(&self, offer_id: u64, action: &str) -> Result<()> {
        let url = format!("{}/tradeoffer/{offer_id}/{action}", self.session.urls().community);
        let form = [("sessionid", self.session.session_id().to_string())];
        let answer: OfferOutcome = json(self.session.post(&url, &form, None).await?).await?;
        match answer.offer_id {
            Some(id) if id == offer_id => Ok(()),
            _ => Err(Error::invalid(format!("Steam did not confirm the {action} of offer {offer_id}"))),
        }
    }

    /// How long a trade with the owner of `link` would be held
    /// (`IEconService/GetTradeHoldDurations`).
    pub async fn hold_durations(&self, link: &TradeLink) -> Result<TradeHold> {
        let mut params = vec![("steamid_target", link.partner.as_u64().to_string())];
        if let Some(token) = &link.token {
            params.push(("trade_offer_access_token", token.clone()));
        }
        let response: ApiResponse<HoldResponse> =
            self.session.api(false, "IEconService", "GetTradeHoldDurations", 1, &params).await?;
        let seconds = |escrow: Option<Escrow>| {
            escrow
                .map(|e| Duration::from_secs(e.escrow_end_duration_seconds))
                .ok_or_else(|| Error::invalid("GetTradeHoldDurations sent no durations"))
        };
        Ok(TradeHold { mine: seconds(response.response.my_escrow)?, theirs: seconds(response.response.their_escrow)? })
    }

    /// This account's trade link, from its trade offer privacy page.
    pub async fn my_trade_link(&self) -> Result<TradeLink> {
        let me = self.session.steam_id();
        let url = format!("{}/profiles/{}/tradeoffers/privacy", self.session.urls().community, me.as_u64());
        let page = self.session.get(&url, &[]).await?;
        if !page.status().is_success() {
            return Err(Error::HttpStatus(page.status().as_u16()));
        }
        trade_link_on_page(&page.text().await?, me)
    }

    /// Acknowledges Steam's trade protection notice, which new trading
    /// accounts must do before they can send offers.
    pub async fn acknowledge_trade_protection(&self) -> Result<()> {
        let url = format!("{}/trade/new/acknowledge", self.session.urls().community);
        let form = [("sessionid", self.session.session_id().to_string()), ("message", "1".into())];
        let response = self.session.post(&url, &form, None).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::HttpStatus(response.status().as_u16()))
        }
    }
}

/// The trade link of `owner` on a page.
pub(crate) fn trade_link_on_page(page: &str, owner: SteamId) -> Result<TradeLink> {
    page.match_indices("tradeoffer/new/?")
        .filter_map(|(i, _)| {
            let start = page[..i].rfind(['"', '\'', ' ', '>']).map_or(0, |s| s + 1);
            TradeLink::parse(&page[start..]).ok()
        })
        .find(|link| link.partner.account_id() == owner.account_id() && link.token.is_some())
        .ok_or_else(|| Error::invalid("no trade link on the trade offer privacy page"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_links() {
        let link =
            TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=79925588&token=Ob27qXzn").unwrap();
        assert_eq!(link.partner, SteamId::from_u64(76561198040191316));
        assert_eq!(link.token.as_deref(), Some("Ob27qXzn"));
        assert_eq!(link.url(), "https://steamcommunity.com/tradeoffer/new/?partner=79925588&token=Ob27qXzn");

        let escaped = TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=1&amp;token=a-b_c").unwrap();
        assert_eq!(escaped.token.as_deref(), Some("a-b_c"));
        let friend = TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=1").unwrap();
        assert_eq!(friend.token, None);
        assert!(TradeLink::parse("https://steamcommunity.com/id/someone").is_err());
        assert!(TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=x").is_err());
    }

    #[test]
    fn own_link_on_the_privacy_page() {
        let page = r#"<input id="trade_offer_access_url" value="https://steamcommunity.com/tradeoffer/new/?partner=1&amp;token=Ob27qXzn">
            <a href="https://steamcommunity.com/tradeoffer/new/?partner=79925588&amp;token=other">a friend's</a>"#;
        let link = trade_link_on_page(page, SteamId::from_account_id(1)).unwrap();
        assert_eq!(link.token.as_deref(), Some("Ob27qXzn"));
        assert!(trade_link_on_page(page, SteamId::from_account_id(2)).is_err());
    }

    #[test]
    fn offer_form_matches_steam_helper() {
        let link = TradeLink::parse("https://steamcommunity.com/tradeoffer/new/?partner=1&token=a-b_c").unwrap();
        let offer = NewOffer::to(link).give(Asset::new(440, 2, 5));
        let form = offer.form("SESSION123");
        let encoded = serde_urlencoded(&form);
        assert_eq!(
            encoded,
            "sessionid=SESSION123&serverid=1&partner=76561197960265729&captcha=&tradeoffermessage=&json_tradeoffer=%7B%22newversion%22%3Atrue%2C%22version%22%3A2%2C%22me%22%3A%7B%22assets%22%3A%5B%7B%22appid%22%3A440%2C%22contextid%22%3A%222%22%2C%22amount%22%3A1%2C%22assetid%22%3A%225%22%7D%5D%2C%22currency%22%3A%5B%5D%2C%22ready%22%3Afalse%7D%2C%22them%22%3A%7B%22assets%22%3A%5B%5D%2C%22currency%22%3A%5B%5D%2C%22ready%22%3Afalse%7D%7D&trade_offer_create_params=%7B%22trade_offer_access_token%22%3A%22a-b_c%22%7D"
        );
    }

    /// Form encoding as reqwest does it.
    fn serde_urlencoded(form: &[(&str, String)]) -> String {
        let request = reqwest::Client::new().post("http://localhost/").form(form).build().unwrap();
        String::from_utf8(request.body().unwrap().as_bytes().unwrap().to_vec()).unwrap()
    }

    #[test]
    fn offers_parse_with_strings_or_numbers() {
        let raw: RawOffer = serde_json::from_str(
            r#"{"tradeofferid":"4127395150","accountid_other":79925588,"trade_offer_state":2,"message":"hi",
                "items_to_give":[{"appid":730,"contextid":"2","assetid":"17034419698","classid":"1","instanceid":"0","amount":"1","missing":false}],
                "is_our_offer":true,"time_created":1,"time_updated":2,"tradeid":"0","escrow_end_date":0,"confirmation_method":2}"#,
        )
        .unwrap();
        let offer = TradeOffer::from(raw);
        assert_eq!(offer.id, 4127395150);
        assert_eq!(offer.partner, SteamId::from_u64(76561198040191316));
        assert_eq!(offer.state, ETradeOfferState::Active);
        assert_eq!(offer.items_to_give[0].asset(), Asset::new(730, 2, 17034419698));
        assert_eq!(offer.trade_id, None);
    }
}
