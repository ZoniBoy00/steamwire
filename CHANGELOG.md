# Changelog

All notable changes to the steamwire crates. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/): before 1.0, a minor version
(0.x) may break the API and a patch version (0.x.y) doesn't.
`steamwire` and `steamwire-protos` are released together with the same
version.

## [Unreleased]

### Added

- The whole of SteamKit2 3.x: the CM client over WebSocket, TCP and UDP,
  every handler, the authentication flows with Steam Guard, the generated
  unified service clients, CDN manifests and chunks, the Web API,
  KeyValues, the LZMA decoder and encoder (VZip output byte-identical to
  SteamKit2's) and the machine ID.
- Tools: the NetHook dump analyzer, the protobuf dumper and the NetHook2
  DLL (`steamwire-tools`, `steamwire-nethook`; not published).
- `native-tls` (default) and `rustls` features for WebSocket and HTTPS.
- Examples: `bot` (reconnecting chat bot with an optional token file),
  `depot_download` (whole depots with parallel chunks and in-place
  updates), `machine_info`, and live checks for anonymous, account and
  two-account use.
- `steamwire-community`: web sessions from a CM access token (renewed
  through the client), trade offers, trade links and holds, inventories,
  mobile confirmations, adding and removing the mobile authenticator
  (`.maFile`), and the account cart with wallet-only purchases. Example
  `community_checks` (read-only live checks).
- Offline handler tests on a loopback client, parser robustness tests,
  cargo-fuzz targets (`fuzz/`) and criterion benchmarks.

### Fixed

- `CredentialsAuthSession::submit_code` treats `DuplicateRequest` as
  success, as SteamKit2 does (found live).
- A `ClientPersonaState` about the logged-on user updates our own persona
  name and state instead of adding us to the friends cache, as SteamKit2's
  `LocalUser` does.
