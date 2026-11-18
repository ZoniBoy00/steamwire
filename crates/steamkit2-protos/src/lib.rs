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

/// The encoded `FileDescriptorSet` of [`steam`], for reflection (decoding
/// messages by name, as the NetHook analyzer does).
pub const STEAM_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/steam/descriptors.bin"));

/// The encoded `FileDescriptorSet` of [`gc`].
pub const GC_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gc/descriptors.bin"));

pub use prost;

use core::future::Future;

/// Sends the calls of the typed service clients in [`steam::services`].
///
/// `steamkit2::SteamClient` implements it over the CM connection.
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
