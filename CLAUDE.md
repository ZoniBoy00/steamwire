# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

steamwire: an async (tokio) Rust port of SteamKit2 (.NET) and its tools. Behaviour,
constants and flows follow the C# SteamKit2 source; when porting or fixing
something, compare against the C# implementation rather than guessing.

## Commands

```sh
cargo build
cargo test --workspace                          # all tests (no network needed)
cargo test -p steamwire --lib auth               # tests whose path matches "auth"
cargo test -p steamwire --test cdn vzip_delta    # one test in an integration test file
cargo clippy --workspace --all-targets          # expected to be warning-free
cargo fmt -p steamwire-protos -p steamwire -p steamwire-community -p steamwire-cs2 -p steamwire-tools   # not --all: steamwire-nethook is vendored
cargo run -p steamwire --example anonymous [-- tcp|udp]   # live logon + PICS
cargo run -p steamwire --example cdn_download    # live CDN download with SHA-1 checks
cargo run -p steamwire --example webapi          # live Web API (JSON, KeyValues, protobuf)
cargo test -p steamwire --test robustness       # parsers never panic (STEAMWIRE_ROUNDS=N for more)
cargo bench -p steamwire --bench codecs         # criterion benchmarks
cargo build -p steamwire --no-default-features --features rustls   # the rustls TLS backend
cargo run -p steamwire --example live_checks     # live anonymous/game-server checks, prints ok/steam/anon/FAIL
cargo run -p steamwire --example account_checks  # needs the user's account (QR); read-only unless --write
cargo run -p steamwire-community --example community_checks  # user's account (QR); read-only web checks
cargo run -p steamwire-cs2 --example cs2_checks  # user's account (QR); read-only CS2 GC checks; shows the account in CS2
cargo run -p steamwire --example account_changes # two of the user's accounts; changes them (see its docs; --refriend re-adds the friend)
cargo run -p steamwire-tools -- nethook <dump-dir> [--seq N] [--emsg NAME]
cargo run -p steamwire-tools -- dump-protos <binary>... -o <dir>
```

`RUST_LOG=steamwire=trace` logs every message; `NETHOOK=1` on the examples
dumps every message to `target/debug/examples/nethook/<time>/`. Those dumps
contain the user's public IP: never commit them or use them as fixtures.
`protoc` is vendored, so no system install is needed. Rust 1.89+.

## Workspace

- `steamwire-protos`: prost types compiled from `proto/` by `build.rs`. Steam
  protos have no `package`, so everything lands in one module each
  (`protos::steam`, `protos::gc`). Also exports the descriptor sets
  (`STEAM_DESCRIPTOR_SET`, `GC_DESCRIPTOR_SET`) used for reflection.
- `steamwire`: the library.
- `steamwire-community`: the web side, not in SteamKit2: `WebSession`
  (cookies from a CM access token, renewed through the client), trade
  offers, inventories, mobile confirmations, adding/removing the
  authenticator (`TwoFactor.*` over the CM, `.maFile`) and wallet
  purchases. Ported in part from SteamHelper-rs (MIT); the checkout follows
  Steam's `checkout.js`. Tested offline against a local HTTP server
  (`tests/common`) and a fake `ServiceTransport`. Never run the
  authenticator, trade or purchase calls on the user's accounts without
  their explicit OK for that run: they move items or money, or can lock the
  account. Don't print tokens, cookies or maFile secrets.
- `steamwire-cs2`: CS2's game coordinator client, not in SteamKit2,
  ported from SteamHelper-rs `steam-client` `gc/cs2` (MIT): inspect links
  and item details, profiles, matches, the CS2 inventory. Built on
  `SteamClient::subscribe_gc`; no reconnecting (a disconnect is
  `Error::Closed`). Tested offline against a fake GC (`Link::Fake` in
  `client.rs`). Connecting plays CS2 on the account: never run it live on
  the user's account without their OK for that run.
- `steamwire-tools`: the NetHook analyzer (dynamic decoding via prost-reflect
  over the descriptor sets) and the ProtobufDumper (library + CLI).
- `steamwire-nethook`: Windows-only NetHook2 DLL adapted from
  CottonIndustries/steamkit-rs (LGPL); empty on other targets. Treat it as
  vendored: don't restyle it, and never inject it into the user's running
  Steam.

## Architecture of `steamwire`

- **Code generation**: `crates/steamwire/build.rs` compiles SteamKit2's
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
  clients in `steamwire::services` (`Player::new(&client)`) are generated
  from the protos' `service` blocks by `steamwire-protos/build.rs` and call
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

Handler behaviour is tested offline in `src/handlers/tests.rs` on a
`client::testing::Loopback`: a real `SteamClient` whose connection is a pair
of channels, so tests dispatch server messages and read back what the client
sends. Add a test there for any handler fix. `fuzz/` holds cargo-fuzz targets
(nightly; excluded from the workspace) mirroring `tests/robustness.rs`.

The integration tests use fixtures in `crates/steamwire/tests/data/`:
- `cdn/`: manifests, chunks and a VZip delta made with cotton's encoder.
- `pics/`: PICS captures.
- `vzip/`: output of SteamKit2's `VZipUtil.Compress` (run under .NET 8) for
  the inputs that `tests/vzip.rs` rebuilds. Its ignored 16 MiB test runs
  with `cargo test --release --test vzip -- --ignored`.

`steamwire-tools/tests/tools.rs` round-trips NetHook dumps and the protobuf
dumper against the compiled descriptor sets. Anything that needs a real
Steam account cannot be tested live, so note it when reporting:
credentials login, Steam Guard and cloud.

## Publishing

The crates are `steamwire-protos`, `steamwire`, `steamwire-community` and `steamwire-cs2` on crates.io (the project
was called steamkit, but that name is taken there); `steamwire-tools` and `steamwire-nethook` are `publish = false`.
`cargo package -p steamwire-protos -p steamwire -p steamwire-community -p steamwire-cs2`
checks them before the dependencies are on crates.io; publish in that order. Nothing has been
published yet: publishing is permanent, so only on the user's say-so.

## Licensing

LGPL-2.1-only. Code ported from other projects keeps its attribution in the
README's References section. For example, `handlers/auth/guard.rs` is ported
from SteamHelper-rs `steam-totp` (MIT).
