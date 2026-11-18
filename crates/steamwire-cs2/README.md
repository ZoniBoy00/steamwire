# steamwire-cs2

Counter-Strike 2's game coordinator for
[steamwire](https://github.com/ZoniBoy00/steamwire): what the game asks its
game coordinator, over a logged-on `steamwire::SteamClient`.

- `Cs2::inspect` / `inspect_str`: an item's details from its inspect link:
  def index, skin (paint index), pattern, float, stickers, keychains,
  StatTrak, name tag. Links that embed the item are read offline.
- `Cs2::player_profile`, `request_matchmaking_stats`: ranks, wins, level,
  commendations, cooldowns.
- `Cs2::match_info`, `recent_matches`, `live_matches`: rounds, scores,
  players, demo links; `ShareCode` encodes and decodes `CSGO-…` codes.
- `Cs2::inventory`: the account's CS2 inventory, kept from the game
  coordinator's cache, with storage units; `next_event` tells what changed.

```rust,no_run
# async fn run(client: steamwire::SteamClient) -> steamwire_cs2::Result<()> {
use steamwire_cs2::Cs2;

let mut cs2 = Cs2::connect(&client).await?;
let item = cs2
    .inspect_str("steam://rungame/730/76561202255233023/+csgo_econ_action_preview%20S76561198084749846A698323590D7935523998312483177")
    .await?;
println!("paint {:?}, pattern {:?}, float {:?}", item.paint_index(), item.pattern(), item.float());
# Ok(()) }
```

Connecting plays CS2 on the account, so Steam shows it in-game, and the
game coordinator keeps one session per account: close CS2 first. Requests
go one at a time, at least `REQUEST_COOLDOWN` apart, since the game
coordinator silently drops requests that come faster. A disconnect ends the
session (`Error::Closed`); connect again with a new client.

Ported from [SteamHelper-rs](https://github.com/saskenuba/SteamHelper-rs)
`steam-client` (MIT, Copyright (c) 2020 Martin Mariano). LGPL-2.1-only.
