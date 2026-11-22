# Roadmap

What's left to do, roughly in order. The [README](README.md) has what's
done, and [CHANGELOG.md](CHANGELOG.md) has what changed. Nothing here is a
promise of an API; items move as they're done or dropped.

## 0.1.1

- Release the packaging fix (in the CHANGELOG under Unreleased): bump the
  version on the maintainer's say-so.
- The read-only live checks below.

## 0.2.0: long-running bots and services

- **Reconnecting, opt-in, in the library.** The `bot` example's loop as a
  feature of `SteamClient`: after a disconnect pick another CM, log on again
  and restore the subscriptions. Requests in flight end with a clear error;
  trades and purchases are never resent.
- **Recovering a `Cs2` session.** Greet the game coordinator again, fetch a
  fresh inventory and tell the consumer when its cache is current again.
  Test a disconnect during a pending request and during an inventory update.
- **Bounded event queues.** Decide what happens when a consumer falls
  behind: ordinary events report the overflow; a lost CS2 inventory update
  needs a fresh snapshot so the app doesn't go on with stale data.
- **Metrics for services**: `tracing` fields or metrics for the connection
  state, reconnects, request durations, timeouts and queue lengths, to tell
  Steam, the game coordinator and the app's own event handling apart.

## Tools built from the examples

- **Depot downloader, library and CLI.** Make the parallel download and the
  reuse of existing chunks in `depot_download` a public API, plus resuming,
  progress, file filters and a dry run that shows what an update would
  download or delete.
- **CS2 inspect and match tool**: an inspect link or share code in, JSON
  out. Items in a link decode locally; the rest asks the game coordinator.
- **CS2 inventory export and diff**: snapshots, added and removed items and
  changed attributes. Find out whether storage unit contents can be asked
  for reliably (today the client shows what the game coordinator sent).
- **Clearer `community` errors**: tell an expired session, a missing
  confirmation, rate limiting and an unknown outcome apart; after a
  disconnect during a trade or purchase, a way to check the outcome before
  trying again.
- **Steam Cloud backup example**: list the files and download a copy.

## Developer experience and upkeep

- **Recipes** for a bot, token renewal, depot downloads and CS2 inspect:
  the Cargo features, credentials needed, and what was run against Steam.
- **Updating the protocol**: one command that moves `proto/` and `steamd/`
  to an upstream commit and shows the changed messages and the generated
  public API's diff.
- **NetHook tooling**: JSON export, filters by message and job ID, and
  redacting sensitive fields (IP, tokens) in dumps meant to be shared.
- **Load and build measurements**: a long session's memory use, and the
  build time with every proto feature.

## Next: what to test

State on 2026-11-20: `account_checks` passes with QR, credentials and the
Steam Guard app confirmation or code; `community_checks` passes; and
`cs2_checks` passes (matchmaking stats included) except `live matches`,
which CS2's game coordinator didn't answer (not counted). Still not run
live, all read-only and optional:

- `community_checks -- --mafile <path>`: the pending confirmations.
- `cs2_checks -- --inspect <link> --profile <friend's steam64> --match
  <CSGO-…>`: inspecting, a friend's profile and a match by share code.
- macOS in practice: the tests and an anonymous logon, and the machine ID
  of Linux and macOS compared with SteamKit2's on the same machine.

Not to run without an explicit decision for that run: trading, accepting
confirmations, purchases, and adding or removing the authenticator.

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

- **Other games' coordinator clients.** The `dota2`, `tf2` and `deadlock`
  proto sets are there, but only CS2 has a typed client. The generic
  `game_coordinator` handler (`call_gc`, `subscribe_gc`) works for all of
  them. A first one would be built around a concrete use (stats or the
  inventory).
- **Uploading screenshots to UFS**, so that `add_screenshot` succeeds.
  SteamKit2 doesn't do it either.
- **More fuzz targets**: the GC packet parser, inspect links and the CS2
  shared-object messages, like the existing ones in `fuzz/`.

## Not planned

- The password web login of SteamHelper-rs: the CM auth sessions replace
  it.
- SteamKit2's `CallbackManager`: requests are `async fn`s, and the rest
  arrives as events.
