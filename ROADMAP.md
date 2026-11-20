# Roadmap

What's left to do, roughly in order. The [README](README.md) has what's
done, and [CHANGELOG.md](CHANGELOG.md) has what changed. Nothing here is a
promise of an API; items move as they're done or dropped.

## Next: what to test

State on 2026-11-18: everything is pushed and CI is green (`bd7dd47`). The
checks below need the maintainer's own account, so they're run by hand,
one at a time.

1. **`cargo run -p steamwire-community --example community_checks`**
   (QR login). Read-only. Expect `ok` on every line and `0 failures`. With
   `-- --mafile <path>` it also lists the pending confirmations.
2. **`cargo run -p steamwire-cs2 --example cs2_checks`** (QR login).
   Read-only, but the account shows as playing CS2 until it logs off.
   Close CS2 first: the game coordinator keeps one session per account.
   The account needs the CS2 license.
   - Expect `ok` for the game coordinator, inventory, matchmaking stats and
     recent matches.
   - `live matches` may time out: the game coordinator doesn't always answer
     it.
   - Optional: `-- --inspect <link> --profile <friend's steam64> --match
     <CSGO-…>`.
3. Note any `FAIL` lines here or as an issue. Then move what passed to the
   README's "Verified against Steam" list and out of this file.

Not to run without an explicit decision for that run: trading, accepting
confirmations, purchases, and adding or removing the authenticator.

## Before the first release (0.1.0)

- **Live checks still open.** These have been tested offline only:
  - `steamwire-community`'s read-only checks (`community_checks`): the web
    session, trade offers, the trade link and holds, the Steam inventory,
    the cart and the mobile confirmation list. The first run was
    interrupted.
  - `steamwire-cs2` against the real game coordinator (`cs2_checks`):
    greeting, the inventory, the matchmaking stats, recent and live matches,
    and optionally inspecting, a friend's profile and a match by share code.
- **Publish** `steamwire-protos`, `steamwire`, `steamwire-community` and
  `steamwire-cs2` to crates.io, in that order, once the checks above pass.
  `cargo package` already passes for all four. Publishing is permanent, so
  it happens only on the maintainer's say-so.

## Verified only in part

- The machine ID on Linux runs in CI but hasn't been compared with
  SteamKit2's output on the same machine; macOS is only compile-checked.
- Changes that can't be undone, or that move items or money, haven't been
  run live: trading, accepting confirmations, wallet purchases, and adding
  or removing the mobile authenticator. They're tested against a local
  server, with Steam's request shapes.
- `share_file`, the legacy chat rooms and `SharedSecretAuthenticator` are
  tested offline only (see Known differences in the README).

## Candidates

Not scheduled; done if there's a use for them.

- **Reconnecting a `Cs2` session.** SteamHelper-rs's client reconnects and
  greets the game coordinator again. In steamwire a disconnect ends the
  session (`Error::Closed`). The session could move to a new client and
  keep its inventory to compare with.
- **Other games' coordinator clients.** The `dota2`, `tf2` and `deadlock`
  proto sets are there, but only CS2 has a typed client. The generic
  `game_coordinator` handler (`call_gc`, `subscribe_gc`) works for all of
  them.
- **Uploading screenshots to UFS**, so that `add_screenshot` succeeds.
  SteamKit2 doesn't do it either.
- **More fuzz targets**: the GC packet parser, inspect links and the CS2
  shared-object messages, like the existing ones in `fuzz/`.

## Not planned

- The password web login of SteamHelper-rs: the CM auth sessions replace
  it.
- SteamKit2's `CallbackManager`: requests are `async fn`s, and the rest
  arrives as events.
