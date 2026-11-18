//! Higher level APIs on top of [`SteamClient`](crate::SteamClient), one
//! module per SteamKit2 handler. Each module adds methods to `SteamClient`
//! and defines a callback enum for the messages it understands.
//!
//! [`Callback::from_packet`] turns any incoming [`Packet`] into a typed
//! callback, which replaces SteamKit2's `CallbackManager`:
//!
//! ```no_run
//! # async fn run(mut events: steamkit2::Events) {
//! use steamkit2::handlers::{Callback, friends::FriendsCallback};
//!
//! while let Some(packet) = events.recv_packet().await {
//!     if let Some(Callback::Friends(FriendsCallback::FriendMsg(msg))) = Callback::from_packet(&packet) {
//!         println!("{}: {}", msg.steamid_from(), String::from_utf8_lossy(msg.message()));
//!     }
//! }
//! # }
//! ```

pub mod apps;
pub mod auth;
pub mod auth_ticket;
pub mod cloud;
pub mod content;
pub mod friends;
pub mod game_coordinator;
pub mod game_server;
pub mod master_server;
pub mod matchmaking;
pub mod networking;
pub mod screenshots;
pub mod unified;
pub mod user;
pub mod user_stats;
pub mod workshop;

use crate::messages::Packet;

/// Generates a callback enum whose variants wrap decoded protobuf bodies,
/// and a `decode_proto` function that picks the variant by `EMsg`.
macro_rules! proto_callbacks {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident($ty:ty) = $emsg:ident, )*
        }
        $(extra { $( $(#[$xmeta:meta])* $xvariant:ident($xty:ty), )* })?
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug)]
        #[non_exhaustive]
        #[allow(clippy::large_enum_variant)]
        pub enum $name {
            $( $(#[$vmeta])* $variant($ty), )*
            $($( $(#[$xmeta])* $xvariant($xty), )*)?
        }

        impl $name {
            #[allow(unused)]
            fn decode_proto(packet: &$crate::messages::Packet) -> Option<Self> {
                match packet.emsg {
                    $( $crate::enums::EMsg::$emsg => packet.decode().ok().map(Self::$variant), )*
                    _ => None,
                }
            }
        }
    };
}
pub(crate) use proto_callbacks;

/// State kept by handlers across messages: the friends cache and the lobby
/// cache (SteamKit2 keeps these inside its handler objects).
#[derive(Debug, Default)]
pub(crate) struct HandlerState {
    pub(crate) friends: friends::FriendsCache,
    pub(crate) lobbies: matchmaking::LobbyCache,
    pub(crate) gc_jobs: game_coordinator::GcJobs,
    pub(crate) auth_tickets: auth_ticket::AuthTicketState,
}

impl HandlerState {
    /// Updates the caches. Returns true if the packet was consumed (a GC
    /// reply handed to [`SteamClient::call_gc`](crate::SteamClient::call_gc)).
    pub(crate) fn observe(&self, client: &crate::SteamClient, packet: &Packet) -> bool {
        self.friends.observe(client, packet);
        self.lobbies.observe(packet);
        self.auth_tickets.observe(packet);
        self.gc_jobs.observe(packet)
    }
}

/// A typed view of an incoming message.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Callback {
    /// [`user`] messages.
    User(user::UserCallback),
    /// [`apps`] messages.
    Apps(apps::AppsCallback),
    /// [`friends`] messages.
    Friends(friends::FriendsCallback),
    /// [`auth_ticket`] messages.
    AuthTicket(auth_ticket::AuthTicketCallback),
    /// [`game_coordinator`] messages.
    GameCoordinator(game_coordinator::GcMessage),
    /// [`game_server`] messages.
    GameServer(game_server::GameServerCallback),
    /// [`master_server`] messages.
    MasterServer(master_server::MasterServerCallback),
    /// [`matchmaking`] messages.
    Matchmaking(matchmaking::MatchmakingCallback),
    /// [`networking`] messages.
    Networking(networking::NetworkingCallback),
    /// [`screenshots`] messages.
    Screenshots(screenshots::ScreenshotsCallback),
    /// [`user_stats`] messages.
    UserStats(user_stats::UserStatsCallback),
    /// [`workshop`] messages.
    Workshop(workshop::WorkshopCallback),
    /// A service notification (`EMsg::ServiceMethod`).
    ServiceMethod(unified::ServiceNotification),
}

impl Callback {
    /// Decodes a packet, or `None` if no handler knows its type (or it
    /// doesn't decode).
    ///
    /// `ClientTicketAuthComplete` is handled by both `SteamAuthTicket` and
    /// `SteamGameServer` in SteamKit2; here it decodes as
    /// [`Callback::AuthTicket`].
    pub fn from_packet(packet: &Packet) -> Option<Callback> {
        None.or_else(|| user::UserCallback::from_packet(packet).map(Callback::User))
            .or_else(|| apps::AppsCallback::from_packet(packet).map(Callback::Apps))
            .or_else(|| friends::FriendsCallback::from_packet(packet).map(Callback::Friends))
            .or_else(|| auth_ticket::AuthTicketCallback::from_packet(packet).map(Callback::AuthTicket))
            .or_else(|| game_coordinator::GcMessage::from_packet(packet).map(Callback::GameCoordinator))
            .or_else(|| game_server::GameServerCallback::from_packet(packet).map(Callback::GameServer))
            .or_else(|| master_server::MasterServerCallback::from_packet(packet).map(Callback::MasterServer))
            .or_else(|| matchmaking::MatchmakingCallback::from_packet(packet).map(Callback::Matchmaking))
            .or_else(|| networking::NetworkingCallback::from_packet(packet).map(Callback::Networking))
            .or_else(|| screenshots::ScreenshotsCallback::from_packet(packet).map(Callback::Screenshots))
            .or_else(|| user_stats::UserStatsCallback::from_packet(packet).map(Callback::UserStats))
            .or_else(|| workshop::WorkshopCallback::from_packet(packet).map(Callback::Workshop))
            .or_else(|| unified::ServiceNotification::from_packet(packet).map(Callback::ServiceMethod))
    }
}
