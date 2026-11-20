//! Read-only live checks of CS2's game coordinator on your own account: logs on by QR code, plays CS2 (Steam shows
//! the account in-game until it logs off: close CS2 first, or the game coordinator drops one of the two sessions),
//! then reads the inventory, the account's matchmaking stats, its recent matches and the live tournament matches.
//! Nothing is changed.
//!
//! Optionally inspects an item (`--inspect <link>`), reads a friend's profile (`--profile <steam64>`) and a match
//! (`--match <share code>`). Prints counts and item numbers only: no tokens.
//!
//! With `NETHOOK=1`, every message is dumped to `nethook/<time>/` next to the executable (read it with
//! `steamwire-tools nethook`). The dumps hold your public IP: don't share or commit them.
//!
//! ```sh
//! cargo run -p steamwire-cs2 --example cs2_checks [-- --inspect <link>] [--profile <steam64>] [--match <code>]
//! ```

use qrcode::render::unicode;
use qrcode::QrCode;
use std::collections::BTreeMap;
use std::sync::Arc;
use steamwire::handlers::auth::AuthSessionDetails;
use steamwire::handlers::user::LogOnDetails;

use steamwire::cs2::{Cs2, Cs2Event, ShareCode};
use steamwire::{Config, NetHookListener, SteamClient, SteamId};

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

fn report<T>(name: &str, result: steamwire::cs2::Result<T>, describe: impl FnOnce(&T) -> String) -> bool {
    match &result {
        Ok(value) => println!("ok    {name}: {}", describe(value)),
        Err(e) => println!("FAIL  {name}: {e}"),
    }
    result.is_ok()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (mut inspect, mut profile, mut share_code) = (None, None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--inspect" => inspect = args.next(),
            "--profile" => profile = args.next(),
            "--match" => share_code = args.next(),
            other => panic!(
                "unknown argument {other}; usage: cs2_checks [--inspect <link>] [--profile <steam64>] [--match <code>]"
            ),
        }
    }
    let profile = profile.map(|id| id.parse().map(SteamId::from_u64)).transpose()?;
    let share_code = share_code.map(|code| code.parse::<ShareCode>()).transpose()?;

    let network_listener = match std::env::var_os("NETHOOK") {
        Some(_) => {
            let listener = NetHookListener::new()?;
            println!("dumping messages to {}", listener.directory().display());
            Some(Arc::new(listener) as _)
        }
        None => None,
    };
    let (client, _events) = SteamClient::connect(Config { network_listener, ..Config::default() }).await?;
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
    let me = client.steam_id().ok_or("no Steam ID after logging on")?;

    let mut failures = 0;
    let mut check = |ok: bool| failures += usize::from(!ok);
    let connected = Cs2::connect(&client).await;
    check(report("game coordinator", connected.as_ref().map(|_| ()).map_err(clone_error), |()| "welcomed".to_string()));
    let Ok(mut cs2) = connected else {
        client.log_off()?;
        println!("{failures} failures");
        return Ok(());
    };

    check(report(
        "inventory",
        cs2.inventory().map(|inventory| inventory.map(|items| items.len())),
        |items| match items {
            Some(items) => format!("{items} items"),
            None => "not sent (yet)".to_string(),
        },
    ));
    // the game coordinator sends them after its welcome; `inventory` read what came so far
    match cs2.matchmaking_stats() {
        Some(stats) => {
            println!("info  stats with the welcome: level {:?}, {} ranks", stats.level(), stats.ranks().count())
        }
        None => println!("info  stats with the welcome: none (yet)"),
    }
    check(report("matchmaking stats", cs2.request_matchmaking_stats().await, |stats| {
        format!("level {:?}, {} ranks", stats.level(), stats.ranks().count())
    }));
    check(report("recent matches", cs2.recent_matches(me).await, |list| format!("{} matches", list.matches().len())));
    // not counted: the game coordinator doesn't always answer it
    match cs2.live_matches().await {
        Ok(list) => println!("ok    live matches: {} matches", list.matches().len()),
        Err(e) => println!("note  live matches: {e} (not counted)"),
    }
    if let Some(link) = inspect {
        check(report("inspect", cs2.inspect_str(&link).await, |item| {
            format!(
                "def {} paint {:?} pattern {:?} float {:?}",
                item.def_index(),
                item.paint_index(),
                item.pattern(),
                item.float()
            )
        }));
    }
    if let Some(player) = profile {
        check(report("profile", cs2.player_profile(player).await, |profile| {
            format!("level {:?}, {} ranks", profile.level(), profile.ranks().count())
        }));
    }
    if let Some(code) = share_code {
        check(report("match", cs2.match_info(&code).await, |list| match list.matches().next() {
            Some(info) => format!("{:?}, score {:?}", info.map(), info.score()),
            None => "not found".to_string(),
        }));
    }

    // what else the game coordinator sent, by kind and message type
    let mut seen = BTreeMap::<String, usize>::new();
    while let Ok(Some(event)) = cs2.try_next_event() {
        let kind = match &event {
            Cs2Event::Other(message) => format!("message {}", message.msg_type()),
            Cs2Event::MatchmakingStats(_) => "matchmaking stats".to_string(),
            Cs2Event::ItemAcquired(_) | Cs2Event::ItemChanged { .. } | Cs2Event::ItemRemoved(_) => "item".to_string(),
            other => format!("{other:?}").split([' ', '(', '{']).next().unwrap_or_default().to_string(),
        };
        *seen.entry(kind).or_default() += 1;
    }
    println!("info  other events: {seen:?}");

    drop(cs2);
    client.log_off()?;
    println!("{failures} failures");
    Ok(())
}

fn clone_error(e: &steamwire::cs2::Error) -> steamwire::cs2::Error {
    steamwire::cs2::Error::Client(steamwire::Error::Other(e.to_string()))
}
