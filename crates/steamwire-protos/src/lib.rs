//! Protobuf messages used by the Steam network, generated with `prost`
//! from the [SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs)
//! definitions.

#![allow(clippy::all, missing_docs, rustdoc::all)]

/// Steam client / CM messages (`steammessages_*.proto`, `enums.proto`, ...).
pub mod steam {
    include!(concat!(env!("OUT_DIR"), "/steam/steam.rs"));
}

/// Game coordinator base messages (`gc.proto`).
pub mod gc {
    include!(concat!(env!("OUT_DIR"), "/gc/gc.rs"));
}

macro_rules! game_set {
    ($feature:literal, $module:ident, $descriptors:ident, $doc:literal) => {
        #[doc = $doc]
        #[cfg(feature = $feature)]
        pub mod $module {
            include!(concat!(env!("OUT_DIR"), "/", $feature, "/", $feature, ".rs"));
        }

        #[doc = concat!("The encoded `FileDescriptorSet` of [`", stringify!($module), "`].")]
        #[cfg(feature = $feature)]
        pub const $descriptors: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/", $feature, "/descriptors.bin"));
    };
}

game_set!("csgo", csgo, CSGO_DESCRIPTOR_SET, "Counter-Strike 2's game coordinator messages (app 730; SteamKit2's `SteamKit2.GC.CSGO.Internal`).");
game_set!("dota2", dota2, DOTA2_DESCRIPTOR_SET, "Dota 2's game coordinator messages (app 570; SteamKit2's `SteamKit2.GC.Dota.Internal`).");
game_set!("tf2", tf2, TF2_DESCRIPTOR_SET, "Team Fortress 2's game coordinator messages (app 440; SteamKit2's `SteamKit2.GC.TF2.Internal`).");
game_set!("deadlock", deadlock, DEADLOCK_DESCRIPTOR_SET, "Deadlock's game coordinator messages (app 1422450; SteamKit2's `SteamKit2.GC.Deadlock.Internal`).");
game_set!("webui", webui, WEBUI_DESCRIPTOR_SET, "The web UI's service messages and typed service clients (`webui::services`; SteamKit2's `SteamKit2.WebUI.Internal`).");

/// The encoded `FileDescriptorSet` of [`steam`], for reflection (decoding
/// messages by name, as the NetHook analyzer does).
pub const STEAM_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/steam/descriptors.bin"));

/// The encoded `FileDescriptorSet` of [`gc`].
pub const GC_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gc/descriptors.bin"));

pub use prost;

use core::future::Future;

/// Sends the calls of the typed service clients in [`steam::services`].
///
/// `steamwire::SteamClient` implements it over the CM connection.
pub trait ServiceTransport: Sync {
    /// The error of a failed call.
    type Error;

    /// Calls `method` (`Service.Method#1`) and decodes its response.
    fn call<Req, Resp>(
        &self,
        method: &'static str,
        request: Req,
    ) -> impl Future<Output = Result<Resp, Self::Error>> + Send
    where
        Req: prost::Message + Send + 'static,
        Resp: prost::Message + Default + Send + 'static;

    /// Sends the notification `method` (`Service.Method#1`) without waiting for a reply.
    fn notify<Req>(&self, method: &'static str, request: Req) -> Result<(), Self::Error>
    where
        Req: prost::Message + Send + 'static;
}
