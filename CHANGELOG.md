# Changelog

All notable changes to the steamwire crates. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/): before 1.0, a minor version
(0.x) may break the API and a patch version (0.x.y) doesn't.

## [Unreleased]

### Added

- SteamKit2 3.x's features (the README lists the differences): the CM
  client over WebSocket, TCP and UDP, every handler, the authentication flows with Steam Guard, the generated
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
- `community` feature: web sessions from a CM access token (renewed
  through the client), trade offers, trade links and holds, inventories,
  mobile confirmations, adding and removing the mobile authenticator
  (`.maFile`), and the account cart with wallet-only purchases. Example
  `community_checks` (read-only live checks).
- `types::ShareCode`: CS2 match share codes, from SteamHelper-rs.
- Features `csgo`, `dota2`, `tf2`, `deadlock` and
  `webui`: the games' GC messages and the web UI services, as SteamKit2
  generates them.
- `SteamClient::subscribe_gc`: one app's GC messages to a subscription
  instead of the events.
- `cs2` feature: CS2's game coordinator client, from SteamHelper-rs:
  inspect links and item details, player profiles and matchmaking stats,
  match info by share code, recent and live matches, and the account's CS2
  inventory with change events. Example `cs2_checks` (read-only live
  checks).
- The default logon `machine_name` and auth `device_friendly_name` are
  "{machine name} (steamwire)", as SteamKit2 appends " (SteamKit2)"; Linux
  lists network adapters in directory order, as SteamKit2 does.
- Offline handler tests on a loopback client, parser robustness tests,
  cargo-fuzz targets (`fuzz/`) and criterion benchmarks.

### Fixed

- `community`: GETs follow redirects within Steam's websites
  with the session's cookies, so the trade link of an account with a
  custom profile URL is found (found live); other redirects are
  `Error::Redirected` with their target, and `is_logged_in` is true only
  when the account page itself answers.
- `cs2`: `request_matchmaking_stats` asks for the account's own
  profile, since CS2's game coordinator no longer answers
  `MatchmakingClient2GCHello` (found live).

- `CredentialsAuthSession::submit_code` treats `DuplicateRequest` as
  success, as SteamKit2 does (found live).
- A `ClientPersonaState` about the logged-on user updates our own persona
  name and state instead of adding us to the friends cache, as SteamKit2's
  `LocalUser` does.
