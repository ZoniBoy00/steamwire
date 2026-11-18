# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

An async (tokio) Rust port of SteamKit2 (.NET) and its tools. Behaviour,
constants and flows follow the C# SteamKit2 source; when porting or fixing
something, compare against the C# implementation rather than guessing.

## Commands

```sh
cargo build
cargo test --workspace                          # all tests (no network needed)
cargo test -p steamkit2 --lib auth               # tests whose path matches "auth"
cargo test -p steamkit2 --test cdn vzip_delta    # one test in an integration test file
cargo clippy --workspace --all-targets          # expected to be warning-free
cargo fmt -p steamkit2-protos -p steamkit2 -p steamkit2-tools   # not --all: steamkit-nethook is vendored
cargo run -p steamkit2 --example anonymous [-- tcp|udp]   # live logon + PICS
cargo run -p steamkit2 --example cdn_download    # live CDN download with SHA-1 checks
cargo run -p steamkit2 --example webapi          # live Web API (JSON, KeyValues, protobuf)
cargo run -p steamkit2 --example live_checks     # live anonymous/game-server checks, prints ok/steam/anon/FAIL
cargo run -p steamkit2 --example account_checks  # needs the user's account (QR); read-only unless --write
cargo run -p steamkit2 --example account_changes # two of the user's accounts; changes them (see its docs; --refriend re-adds the friend)
cargo run -p steamkit2-tools -- nethook <dump-dir> [--seq N] [--emsg NAME]
cargo run -p steamkit2-tools -- dump-protos <binary>... -o <dir>
```

`RUST_LOG=steamkit2=trace` logs every message; `NETHOOK=1` on the examples
dumps every message to `target/debug/examples/nethook/<time>/`. Those dumps
contain the user's public IP: never commit them or use them as fixtures.
`protoc` is vendored, so no system install is needed. Rust 1.89+.

## Workspace

- `steamkit2-protos`: prost types compiled from `proto/` by `build.rs`. Steam
  protos have no `package`, so everything lands in one module each
  (`protos::steam`, `protos::gc`). Also exports the descriptor sets
  (`STEAM_DESCRIPTOR_SET`, `GC_DESCRIPTOR_SET`) used for reflection.
- `steamkit2`: the library.
- `steamkit2-tools`: the NetHook analyzer (dynamic decoding via prost-reflect
  over the descriptor sets) and the ProtobufDumper (library + CLI).
- `steamkit-nethook`: Windows-only NetHook2 DLL adapted from
  CottonIndustries/steamkit-rs (LGPL); empty on other targets. Treat it as
  vendored: don't restyle it, and never inject it into the user's running
  Steam.

## Architecture of `steamkit2`

- **Code generation**: `crates/steamkit2/build.rs` compiles SteamKit2's
  `.steamd` files in `steamd/` into enums and `Msg*` structs
  (`messages::generated`). Enums are newtypes with associated constants
  (`EMsg(pub u32)`, `EMsg::ClientLogon`), not Rust enums, because of aliases
  and unknown values. It also generates `describe_body(emsg, data)` used by
  the tools.
- **Client** (`client/`): one cloneable `SteamClient` (an `Arc<Inner>`).
  `SteamClient::connect(Config)` returns the client plus an `Events` stream.
  A reader task calls `dispatch_raw` → `dispatch`: it unpacks `Multi`, feeds
  the `NetworkListener`, updates the handler caches (`handlers::HandlerState`),
  completes jobs (request/response matched by job ID, via `call`), and sends
  everything else out as events. Requests are `async fn`s that resolve to
  their response, instead of SteamKit2's callback manager.
- **Handlers** (`handlers/<name>/`): one module per SteamKit2 handler, each
  adding methods to `SteamClient` through `impl SteamClient` blocks. Unified
  messages go through `call_service("Service.Method#1", request)`; the typed
  clients in `steamkit2::services` (`Player::new(&client)`) are generated
  from the protos' `service` blocks by `steamkit2-protos/build.rs` and call
  it through the `ServiceTransport` trait. Errors
  from Steam are `Error::Steam` or `Error::SteamMessage`; use
  `Error::result()` to match on the `EResult`.
- **Transports** (`net/`): WebSocket, TCP and UDP. TCP and UDP share the
  channel-encryption handshake in `net/envelope.rs`. UDP is SteamKit2's
  reliable protocol (`udp.rs`, with loopback tests).
- **Messages** (`messages/`): `Packet` (parsed header + body bytes),
  `ProtoMsg<T>` / `ClientMsg<T>` for sending, GC packets in `gc.rs`.
- **Other modules**:
  - `cdn/`: manifests (protobuf and binary v4) and chunks.
  - `util/`:
    - LZMA decoder and encoder (`lzma_encoder.rs`, the LZMA SDK's C#
      encoder) ported from SteamKit2, VZip/VZstd/delta; `vzip_compress`
      must stay byte-identical to SteamKit2's (`tests/vzip.rs`).
    - crypto, `os.rs` (OS type and version), `net.rs` (IP helpers).
    - `hardware.rs`: the machine ID from real hardware; `Config::machine_info_provider` can replace it.
  - `webapi/`: JSON, KeyValues and protobuf Web API calls, plus the CM directory.
  - `types/`: `SteamId`, KeyValues text and binary.
- `#![deny(unsafe_code)]`: the only allowed unsafe is the Windows platform
  module in `util/hardware.rs`.

## Tests

The integration tests use fixtures in `crates/steamkit2/tests/data/`:
- `cdn/`: manifests, chunks and a VZip delta made with cotton's encoder.
- `pics/`: PICS captures.
- `vzip/`: output of SteamKit2's `VZipUtil.Compress` (run under .NET 8) for
  the inputs that `tests/vzip.rs` rebuilds. Its ignored 16 MiB test runs
  with `cargo test --release --test vzip -- --ignored`.

`steamkit2-tools/tests/tools.rs` round-trips NetHook dumps and the protobuf
dumper against the compiled descriptor sets. Anything that needs a real
Steam account cannot be tested live, so note it when reporting:
credentials login, Steam Guard and cloud.

## Publishing

The crates are `steamkit2-protos` and `steamkit2` on crates.io (`steamkit`
was taken); `steamkit2-tools` and `steamkit-nethook` are `publish = false`.
`cargo package -p steamkit2-protos -p steamkit2` checks both before the
protos are on crates.io; publish the protos first. Nothing has been
published yet: publishing is permanent, so only on the user's say-so.

## Licensing

LGPL-2.1-only. Code ported from other projects keeps its attribution in the
README's References section. For example, `handlers/auth/guard.rs` is ported
from SteamHelper-rs `steam-totp` (MIT).
