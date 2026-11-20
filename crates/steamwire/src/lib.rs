//! An async Rust library for talking to the Steam network, modelled on
//! [SteamKit2](https://github.com/SteamRE/SteamKit).
//!
//! One cloneable [`SteamClient`] connects to a Steam CM server. Requests are
//! `async fn`s that resolve to Steam's response, and everything else Steam
//! sends arrives on the [`Events`] stream.
//!
//! ```no_run
//! use steamwire::{Config, SteamClient};
//!
//! # async fn run() -> steamwire::Result<()> {
//! let (client, _events) = SteamClient::connect(Config::default()).await?;
//! client.log_on_anonymous().await?;
//! println!("logged on as {}", client.steam_id().unwrap());
//! # Ok(()) }
//! ```
//!
//! # Where things are
//!
//! - [`client`]: connecting, logging on, events, jobs.
//! - [`handlers`]: SteamKit2's handlers as methods on [`SteamClient`]:
//!   [`handlers::auth`] (QR and password logon, Steam Guard), friends, apps
//!   and PICS, user stats, cloud, workshop, the game coordinator, ...
//! - [`services`]: typed clients for the unified services (`Player`,
//!   `Econ`, ...), generated from Steam's protos ([`protos`]).
//! - [`cdn`]: depot manifests and chunks; [`webapi`]: Steam's Web API.
//! - [`types`]: [`SteamId`], KeyValues ([`kv`]), [`GameId`], [`JobId`].
//! - [`net`] and [`messages`]: transports and packets, for lower level use.
//!
//! # Features
//!
//! - `native-tls` (default) or `rustls`: TLS for WebSocket, the Web API and
//!   the CDN. Without either, only TCP and UDP connections work.
//! - `community`: Steam's web side (`steamwire::community`): web sessions, trade
//!   offers, inventories, mobile confirmations, the mobile authenticator and
//!   wallet purchases.
//! - `cs2`: Counter-Strike 2's game coordinator client (`steamwire::cs2`): inspect
//!   links, profiles, matches, the CS2 inventory.
//! - `csgo`, `dota2`, `tf2`, `deadlock`, `webui`: further proto sets
//!   ([`protos`]).
//!
//! The [README](https://github.com/ZoniBoy00/steamwire#readme) has more
//! examples and the differences from SteamKit2.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// Unsafe only in the Windows machine ID queries (`util::hardware`).
#![deny(unsafe_code)]

pub mod cdn;
pub mod client;
#[cfg(feature = "community")]
pub mod community;
#[cfg(feature = "cs2")]
pub mod cs2;
pub mod discovery;
pub mod enums;
mod error;
pub mod handlers;
pub mod messages;
pub mod net;
pub mod protos;
pub mod types;
pub mod util;
pub mod webapi;

pub use client::{Config, Event, Events, JobReceiver, SteamClient};
pub use enums::{EAccountType, EMsg, EPersonaState, EResult, EUniverse};
pub use error::{Error, Result};
pub use messages::{Packet, ProtoMsg};
pub use net::{CmServer, NetHookListener, NetworkListener, ProtocolType};
pub use protos::steam::services;
pub use types::kv;
pub use types::{GameId, JobId, SteamId};
