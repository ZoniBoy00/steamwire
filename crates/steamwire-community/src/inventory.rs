//! Inventories (`https://steamcommunity.com/inventory/{steamid}/{app}/{context}`),
//! read page by page as SteamHelper-rs's `steam-trading` (MIT, Copyright (c)
//! 2020 Martin Mariano) does.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use steamwire::SteamId;

use crate::error::{Error, Result};
use crate::session::{json, WebSession};
use crate::util::{flexible_bool, flexible_u64, optional_flexible_u64};

/// Items per inventory page.
const PAGE_SIZE: u32 = 1000;
/// At most this many pages are read, in case Steam's paging never ends.
const MAX_PAGES: usize = 100;

/// An item in a trade: which app and context it is in, its ID and how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Asset {
    /// The app the item belongs to, such as 730 for Counter-Strike 2.
    pub appid: u32,
    /// The inventory context, usually 2 for games and 6 for Steam items.
    pub contextid: u64,
    /// The asset ID.
    pub assetid: u64,
    /// How many, for stackable items.
    pub amount: u64,
}

impl Asset {
    /// One of the asset `assetid`.
    pub fn new(appid: u32, contextid: u64, assetid: u64) -> Self {
        Self { appid, contextid, assetid, amount: 1 }
    }

    /// `amount` of a stackable asset.
    pub fn with_amount(mut self, amount: u64) -> Self {
        self.amount = amount;
        self
    }
}

/// What an item is: its names and whether it can be traded or sold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct Description {
    /// The name shown.
    #[serde(default)]
    pub name: String,
    /// The name on the Community Market.
    #[serde(default)]
    pub market_hash_name: String,
    /// The type line, such as "Mil-Spec Grade Rifle".
    #[serde(default, rename = "type")]
    pub item_type: String,
    /// It can be traded now.
    #[serde(default, deserialize_with = "flexible_bool")]
    pub tradable: bool,
    /// It can be sold on the market.
    #[serde(default, deserialize_with = "flexible_bool")]
    pub marketable: bool,
    /// It stacks with items of the same class.
    #[serde(default, deserialize_with = "flexible_bool")]
    pub commodity: bool,
    /// The icon's path on Steam's CDN.
    #[serde(default)]
    pub icon_url: String,
}

/// An item of an inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Item {
    /// The item, as trade offers take it.
    pub asset: Asset,
    /// Its class.
    pub classid: u64,
    /// Its instance.
    pub instanceid: u64,
    /// What it is, if Steam described it.
    pub description: Option<Description>,
}

#[derive(Deserialize)]
struct RawAsset {
    appid: u32,
    #[serde(deserialize_with = "flexible_u64")]
    contextid: u64,
    #[serde(deserialize_with = "flexible_u64")]
    assetid: u64,
    #[serde(deserialize_with = "flexible_u64")]
    classid: u64,
    #[serde(default, deserialize_with = "flexible_u64")]
    instanceid: u64,
    #[serde(deserialize_with = "flexible_u64")]
    amount: u64,
}

#[derive(Deserialize)]
struct RawDescription {
    #[serde(deserialize_with = "flexible_u64")]
    classid: u64,
    #[serde(default, deserialize_with = "flexible_u64")]
    instanceid: u64,
    #[serde(flatten)]
    description: Description,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default, deserialize_with = "flexible_bool")]
    success: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
    #[serde(default)]
    descriptions: Vec<RawDescription>,
    #[serde(default, deserialize_with = "flexible_bool")]
    more_items: bool,
    #[serde(default, deserialize_with = "optional_flexible_u64")]
    last_assetid: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

/// Inventories on steamcommunity.com.
#[derive(Clone, Debug)]
pub struct Inventories {
    session: WebSession,
}

impl Inventories {
    /// Uses `session`.
    pub fn new(session: WebSession) -> Self {
        Self { session }
    }

    /// This account's items of `appid` in `contextid`.
    pub async fn own(&self, appid: u32, contextid: u64) -> Result<Vec<Item>> {
        self.of(self.session.steam_id(), appid, contextid).await
    }

    /// `owner`'s items of `appid` in `contextid`. A private inventory fails
    /// with HTTP status 403.
    pub async fn of(&self, owner: SteamId, appid: u32, contextid: u64) -> Result<Vec<Item>> {
        let url = format!("{}/inventory/{}/{appid}/{contextid}", self.session.urls().community, owner.as_u64());
        let mut items = Vec::new();
        let mut starts = HashSet::new();
        let mut start = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("l", "english".to_string()), ("count", PAGE_SIZE.to_string())];
            if let Some(start) = start {
                query.push(("start_assetid", u64::to_string(&start)));
            }
            let response = self.session.get(&url, &query).await?;
            // A private or missing inventory is `null` with an error status.
            if !response.status().is_success() {
                return Err(Error::HttpStatus(response.status().as_u16()));
            }
            let page: Page = json(response).await?;
            if !page.success {
                return Err(Error::Steam { result: steamwire::EResult::Fail, message: page.error });
            }
            let described: HashMap<(u64, u64), Description> =
                page.descriptions.into_iter().map(|d| ((d.classid, d.instanceid), d.description)).collect();
            items.extend(page.assets.into_iter().map(|a| Item {
                asset: Asset { appid: a.appid, contextid: a.contextid, assetid: a.assetid, amount: a.amount },
                classid: a.classid,
                instanceid: a.instanceid,
                description: described.get(&(a.classid, a.instanceid)).cloned(),
            }));
            match (page.more_items, page.last_assetid) {
                (true, Some(last)) if starts.insert(last) => start = Some(last),
                (true, _) => return Err(Error::invalid("the inventory's paging repeats or has no next page")),
                (false, _) => return Ok(items),
            }
        }
        Err(Error::invalid(format!("the inventory has more than {MAX_PAGES} pages")))
    }
}
