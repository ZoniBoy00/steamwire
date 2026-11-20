# Roadmap

What's left to do, roughly in order. The [README](README.md) has what's
done, and [CHANGELOG.md](CHANGELOG.md) has what changed. Nothing here is a
promise of an API; items move as they're done or dropped.

## Next: what to test

State on 2026-11-20: `community_checks` passes, and `cs2_checks` passes
except `live matches`, which CS2's game coordinator didn't answer (not
counted). Still not run live, all read-only and optional:

- `community_checks -- --mafile <path>`: the pending confirmations.
- `cs2_checks -- --inspect <link> --profile <friend's steam64> --match
  <CSGO-…>`: inspecting, a friend's profile and a match by share code.

Not to run without an explicit decision for that run: trading, accepting
confirmations, purchases, and adding or removing the authenticator.

## Before the first release (0.1.0)

- **Publish** `steamwire` to crates.io, now that the live checks pass (the
  web side and CS2 are its `community` and `cs2` features).
  `cargo package` already passes for all four. Publishing is permanent, so
  it happens only on the maintainer's say-so: the release workflow
  publishes on the first green CI run on main once it's enabled again
  (`gh workflow enable Release`; the `CARGO_REGISTRY_TOKEN` secret is set).

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
