# steamwire-community

Steam's web side for [steamwire](https://github.com/ZoniBoy00/steamwire):
what goes through steamcommunity.com, the store and the Web API instead of
the client protocol.

- `WebSession`: the login cookies and access token, made from a logged-on
  `steamwire::SteamClient` and renewed through it about daily.
- `TradeOffers`, `TradeLink`, `Inventories`: list, send, accept, decline and
  cancel trade offers; trade links, trade holds and inventories.
- `Confirmations`: mobile confirmations of trades and market listings, with
  the authenticator's `identity_secret`.
- `authenticator`: add (and finalize) or remove the Steam Guard mobile
  authenticator over the client connection, with Steam Desktop
  Authenticator's `.maFile` format.
- `Store`: the account cart, and buying it with the Steam Wallet only.

```rust,no_run
# async fn run(client: steamwire::SteamClient, refresh_token: String) -> steamwire_community::Result<()> {
use steamwire_community::{OfferFilter, TradeOffers, WebSession};

let session = WebSession::from_client(&client, refresh_token).await?;
for offer in TradeOffers::new(session).list(OfferFilter::default()).await?.received {
    println!("offer {} from {}: {:?}", offer.id, offer.partner.steam3(), offer.state);
}
# Ok(()) }
```

Adding an authenticator replaces the account's current one, and removing it
can hold trades for days: save the `.maFile` (it has the revocation code)
before finalizing. Purchases are wallet-only, refused before anything is
charged if the wallet can't pay all of it, and never retried.

Ported in part from [SteamHelper-rs](https://github.com/saskenuba/SteamHelper-rs)
(MIT, Copyright (c) 2020 Martin Mariano). LGPL-2.1-only.
