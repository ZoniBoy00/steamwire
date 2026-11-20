//! An async Rust library for talking to the Steam network, modelled on
//! [SteamKit2](https://github.com/SteamRE/SteamKit).
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
