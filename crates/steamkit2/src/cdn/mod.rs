//! Content delivery (SteamKit2's `CDN` namespace): content servers, the CDN
//! client, depot manifests and chunk processing.

pub mod chunk;
mod client;
mod manifest;
mod server;

pub use client::{build_url, is_private_address, CdnClient};
pub use manifest::{ChunkData, DepotManifest, FileData};
pub use server::{ConnectionProtocol, Server};
