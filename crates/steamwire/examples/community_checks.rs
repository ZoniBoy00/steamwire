//! Read-only live checks of the web side on your own account: logs on by
//! QR code, makes a web session and reads the trade offers, the trade link,
//! the Steam inventory and the cart. Nothing is sent, accepted, bought or
//! changed. With `--mafile <path>` it also lists the pending mobile
//! confirmations (without answering them).
//!
//! Prints counts only: no tokens, cookies, trade link token or secrets.
//!
//! ```sh
//! cargo run -p steamwire-community --example community_checks [-- --mafile <path>]
//! ```

use qrcode::render::unicode;
use qrcode::QrCode;
use steamwire::community::{
    Confirmations, Inventories, MobileAuth, OfferFilter, Store, TradeLink, TradeOffers, WebSession,
};
use steamwire::handlers::auth::AuthSessionDetails;
use steamwire::handlers::user::LogOnDetails;
use steamwire::{Config, SteamClient};

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

fn report<T>(name: &str, result: steamwire::community::Result<T>, describe: impl FnOnce(&T) -> String) -> bool {
    match &result {
        Ok(value) => println!("ok    {name}: {}", describe(value)),
        Err(e) => println!("FAIL  {name}: {e}"),
    }
    result.is_ok()
}

#[tokio::main]
async fn main() -> steamwire::community::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut mafile = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mafile" => mafile = args.next(),
            other => panic!("unknown argument {other}; usage: community_checks [--mafile <path>]"),
        }
    }

    let (client, _events) = SteamClient::connect(Config::default()).await?;
    let mut qr = client.begin_auth_session_via_qr(&AuthSessionDetails::default()).await?;
    print_qr(qr.challenge_url());
    println!("Scan it with the Steam mobile app.");
    let tokens = qr.wait(print_qr).await?;
    client
        .log_on(LogOnDetails {
            username: tokens.account_name.clone(),
            access_token: Some(tokens.refresh_token.clone()),
            ..LogOnDetails::default()
        })
        .await?;

    let session = WebSession::from_client(&client, tokens.refresh_token).await?;
    let mut failures = 0;
    let mut check = |ok: bool| failures += usize::from(!ok);

    check(report("web session", session.is_logged_in().await, |ok| format!("logged in: {ok}")));
    let offers = TradeOffers::new(session.clone());
    check(report("trade offers", offers.list(OfferFilter::default()).await, |list| {
        format!("{} sent, {} received (active)", list.sent.len(), list.received.len())
    }));
    let link = offers.my_trade_link().await;
    check(report("trade link", link.as_ref().map(TradeLink::clone).map_err(clone_error), |link| {
        format!("found, partner {}", link.partner.steam3())
    }));
    if let Ok(link) = &link {
        check(report("trade hold", offers.hold_durations(link).await, |hold| {
            format!("{} s mine, {} s theirs", hold.mine.as_secs(), hold.theirs.as_secs())
        }));
    }
    check(report("Steam inventory", Inventories::new(session.clone()).own(753, 6).await, |items| {
        format!("{} items", items.len())
    }));
    check(report("cart", Store::new(session.clone()).cart().await, |cart| format!("{} lines", cart.items.len())));

    if let Some(path) = mafile {
        let auth = MobileAuth::load(&path)?;
        let mut confirmations = Confirmations::new(session.clone(), auth.identity_secret()?);
        if !auth.device_id.is_empty() {
            confirmations = confirmations.with_device_id(auth.device_id.clone());
        }
        check(report("confirmations", confirmations.list().await, |list| format!("{} pending", list.len())));
    }

    client.log_off()?;
    println!("{failures} failures");
    Ok(())
}

fn clone_error(e: &steamwire::community::Error) -> steamwire::community::Error {
    steamwire::community::Error::InvalidResponse(e.to_string())
}
