//! Counter-Strike 2 over its game coordinator (the `cs2` feature), through a
//! [`Cs2`] session: inspect links and the item details (pattern, float,
//! stickers) the game coordinator gives for them, players' profiles and
//! ranks, matches and their share codes, and the account's inventory.
//!
//! A session runs over a logged-on [`crate::SteamClient`] whose account
//! owns CS2 (the free license). [`Cs2::connect`] plays CS2 and greets the game
//! coordinator until it welcomes the account; the welcome carries the
//! inventory, which the session then keeps as the game coordinator changes
//! it, and gives only while it is current: [`Cs2::inventory`] is `None` from
//! [`Cs2Event::InventoryUnavailable`] until [`Cs2Event::InventoryLoaded`].
//! Requests go one at a time, [`REQUEST_COOLDOWN`] (1.1 s) apart: the game
//! coordinator drops those that come faster. What no request waits for comes
//! from [`Cs2::next_event`].
//!
//! ```no_run
//! # async fn run(client: steamwire::SteamClient) -> Result<(), Box<dyn std::error::Error>> {
//! use steamwire::SteamId;
//! use steamwire::cs2::{Cs2, Cs2Event, ShareCode};
//!
//! let mut cs2 = Cs2::connect(&client).await?;
//! let link = concat!(
//!     "steam://rungame/730/76561202255233023/+csgo_econ_action_preview",
//!     "%20S76561198084749846A698323590D7935523998312483177",
//! );
//! let item = cs2.inspect_str(link).await?;
//! println!("pattern {:?}, float {:?}", item.pattern(), item.float());
//!
//! // a friend of the account who plays CS2
//! let profile = cs2.player_profile(SteamId::from_u64(76561198084749846)).await?;
//! println!("level {:?}, ranks {:?}", profile.level(), profile.ranks().collect::<Vec<_>>());
//!
//! let code: ShareCode = "CSGO-vGqmL-vb2x2-nTQtU-e9dco-qWHzN".parse()?;
//! for info in cs2.match_info(&code).await?.matches() {
//!     println!("{:?}: {:?}, demo {:?}", info.map(), info.score(), info.demo_url());
//! }
//!
//! if let Some(inventory) = cs2.inventory()? {
//!     println!("{} items", inventory.len());
//! }
//! loop {
//!     match cs2.next_event().await? {
//!         Cs2Event::ItemAcquired(item) => println!("acquired {}", item.id()),
//!         Cs2Event::ItemRemoved(item) => println!("removed {}", item.id()),
//!         Cs2Event::SessionLost => println!("lost the session, greeting again"),
//!         _ => {}
//!     }
//! }
//! # }
//! ```
//!
//! Inspect links that embed their item decode offline, without a session:
//! [`EmbeddedItem::details`], or [`Cs2::inspect`], which then sends nothing.
//! The game coordinator answers profiles of the account's friends only,
//! node-globaloffensive says.
//!
//! Not covered: the inventories of other accounts (list them from Steam's web
//! inventory, e.g. with the `community` feature, then inspect each item's link),
//! the items inside storage units (loading a unit isn't built: the
//! [`Inventory`] has those the game coordinator sent unasked), and whatever
//! changes the account, such as moving items in or out of storage units, name
//! tags or stickers.
//!
//! Ported from SteamHelper-rs's `steam-client` (`gc/cs2`, MIT, Copyright (c)
//! 2020 Martin Mariano), which follows node-globaloffensive; see the README's
//! References. The messages are [`crate::protos::csgo`].

mod client;
mod error;
mod inventory;
mod item;
mod link;
mod matches;
mod profile;

pub use crate::types::{ShareCode, ShareCodeError};
pub use client::{Cs2, Cs2Event, APP_ID, EVENT_CAPACITY, GREET_TIMEOUT, REQUEST_COOLDOWN, REQUEST_TIMEOUT};
pub use error::{Error, Result};
pub use inventory::{Inventory, InventoryItem, InventoryItemError};
pub use item::{ItemDetails, ItemDetailsError, Keychain, StatTrak, Sticker};
pub use link::{EmbeddedItem, InspectLink, InspectLinkError};
pub use matches::{MatchInfo, MatchList};
pub use profile::{Commendations, Penalty, PlayerProfile, Rank};
