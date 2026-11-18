//! Developer tools for steamkit, ported from SteamKit2's `Resources`:
//!
//! - [`nethook`]: reads NetHook dumps and decodes every message into a
//!   tree (NetHookAnalyzer2, as a library and CLI rather than a GUI).
//! - [`protodump`]: finds serialized protobuf descriptors in binaries and
//!   writes them back as `.proto` files (ProtobufDumper).

pub mod nethook;
pub mod protodump;
pub mod tree;
mod wire;
