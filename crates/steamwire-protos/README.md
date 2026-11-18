# steamwire-protos

Steam's protobuf messages for [steamwire](https://github.com/ZoniBoy00/steamwire),
generated with prost at build time (`protoc` is vendored).

- `steam`: the client and unified service messages, from
  [SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs),
  with the typed service clients generated from their `service` blocks
  (`steam::services`) and the descriptor set (`STEAM_DESCRIPTOR_SET`) for
  reflection.
- `gc`: the game coordinator's common messages (`GC_DESCRIPTOR_SET`).

Each game's coordinator messages and the web UI services are behind a
feature of the same name, as SteamKit2 has them
(`SteamKit2.GC.*.Internal`, `SteamKit2.WebUI.Internal`):

| Feature | Module | Descriptor set |
|---|---|---|
| `csgo` | `csgo` (Counter-Strike 2, app 730) | `CSGO_DESCRIPTOR_SET` |
| `dota2` | `dota2` (Dota 2, app 570) | `DOTA2_DESCRIPTOR_SET` |
| `tf2` | `tf2` (Team Fortress 2, app 440) | `TF2_DESCRIPTOR_SET` |
| `deadlock` | `deadlock` (Deadlock, app 1422450) | `DEADLOCK_DESCRIPTOR_SET` |
| `webui` | `webui` (the web UI services, with `webui::services`) | `WEBUI_DESCRIPTOR_SET` |

The game and web UI sets are vendored from
[SteamTracking/Protobufs](https://github.com/SteamTracking/Protobufs) at
`89b77382`, the commit SteamKit2 pins. The build strips Valve's own
descriptor options, which only Valve's fork of `descriptor.proto` defines.

LGPL-2.1-only.
