//! The build script: the enums and message bodies from SteamKit2's `.steamd`
//! files (`steamd.rs`), and the protobuf messages from `proto/`
//! (`protos.rs`).

mod protos;
mod steamd;

fn main() {
    steamd::run();
    protos::run();
}
