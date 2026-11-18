//! A long-running bot: logs on, answers friends' chat messages, and
//! reconnects and logs on again whenever the connection drops, waiting
//! longer after each failure (5 s, 10 s, … up to 5 min).
//!
//! The first logon is by QR code. With `--token-file <path>` the refresh
//! token is saved there, so later runs log on without the app until it
//! expires; the file grants access to the account like a password, so keep
//! it private. Without the flag the token stays in memory only.
//!
//! ```sh
//! cargo run -p steamwire --example bot [-- --token-file <path>]
//! ```

use std::path::PathBuf;
use std::time::Duration;

use qrcode::render::unicode;
use qrcode::QrCode;
use steamwire::enums::{EChatEntryType, EPersonaState, EResult};
use steamwire::handlers::auth::AuthSessionDetails;
use steamwire::handlers::friends::FriendsCallback;
use steamwire::handlers::user::LogOnDetails;
use steamwire::handlers::Callback;
use steamwire::{Config, Event, SteamClient, SteamId};

/// What logs on: the account name and its refresh token.
#[derive(Clone)]
struct Credentials {
    account_name: String,
    refresh_token: String,
}

struct TokenStore {
    path: Option<PathBuf>,
}

impl TokenStore {
    fn load(&self) -> Option<Credentials> {
        let text = std::fs::read_to_string(self.path.as_ref()?).ok()?;
        let (account_name, refresh_token) = text.trim().split_once('\n')?;
        Some(Credentials { account_name: account_name.into(), refresh_token: refresh_token.trim().into() })
    }

    fn save(&self, credentials: &Credentials) {
        if let Some(path) = &self.path {
            let text = format!("{}\n{}\n", credentials.account_name, credentials.refresh_token);
            if let Err(e) = std::fs::write(path, text) {
                eprintln!("could not save the token to {}: {e}", path.display());
            }
        }
    }

    fn forget(&self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Logon results that mean the token itself is no good.
fn token_rejected(error: &steamwire::Error) -> bool {
    matches!(
        error.result(),
        Some(
            EResult::InvalidPassword
                | EResult::AccessDenied
                | EResult::Expired
                | EResult::Revoked
                | EResult::InvalidSignature
        )
    )
}

async fn authenticate(client: &SteamClient) -> steamwire::Result<Credentials> {
    let mut session = client.begin_auth_session_via_qr(&AuthSessionDetails::default()).await?;
    print_qr(session.challenge_url());
    println!("Scan it with the Steam mobile app.");
    let tokens = session.wait(print_qr).await?;
    Ok(Credentials { account_name: tokens.account_name, refresh_token: tokens.refresh_token })
}

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

/// Answers a chat message.
fn reply(client: &SteamClient, from: SteamId, message: &str) -> steamwire::Result<()> {
    let answer = match message.trim() {
        "!ping" => "pong".to_string(),
        "!help" => "commands: !ping, !help; anything else is echoed back".to_string(),
        other => format!("you said: {other}"),
    };
    client.send_chat_message(from, EChatEntryType::ChatMsg, &answer)
}

/// One connection, from connecting to disconnecting. `Ok` means the bot was
/// logged on, so the next reconnect starts with a short wait again.
async fn run_once(store: &TokenStore, credentials: &mut Option<Credentials>) -> steamwire::Result<()> {
    let (client, mut events) = SteamClient::connect(Config::default()).await?;
    let current = match credentials.clone() {
        Some(current) => current,
        None => {
            let fresh = authenticate(&client).await?;
            store.save(&fresh);
            *credentials = Some(fresh.clone());
            fresh
        }
    };
    let logged_on = client
        .log_on(LogOnDetails {
            username: current.account_name.clone(),
            access_token: Some(current.refresh_token.clone()),
            ..LogOnDetails::default()
        })
        .await;
    if let Err(e) = &logged_on {
        if token_rejected(e) {
            println!("Steam rejected the saved token; a new QR code comes next");
            store.forget();
            *credentials = None;
        }
    }
    println!("logged on as {}", logged_on?.steam_id.steam3());
    client.set_persona_state(EPersonaState::Online)?;

    while let Some(event) = events.recv().await {
        match event {
            Event::Message(packet) => {
                if let Some(Callback::Friends(FriendsCallback::FriendMsg(msg))) = Callback::from_packet(&packet) {
                    let text = String::from_utf8_lossy(msg.message()).trim_end_matches('\0').to_string();
                    if msg.chat_entry_type() == EChatEntryType::ChatMsg.0 && !text.is_empty() {
                        let from = SteamId::from_u64(msg.steamid_from());
                        println!("{} says: {text}", from.steam3());
                        reply(&client, from, &text)?;
                    }
                }
            }
            Event::Disconnected { user_initiated } => {
                println!("disconnected (by us: {user_initiated})");
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--token-file" => path = args.next().map(PathBuf::from),
            other => {
                eprintln!("unknown argument {other}; usage: bot [--token-file <path>]");
                std::process::exit(2);
            }
        }
    }
    let store = TokenStore { path };
    let mut credentials = store.load();

    const FIRST: Duration = Duration::from_secs(5);
    const MAX: Duration = Duration::from_secs(300);
    let mut wait = FIRST;
    loop {
        let delay = match run_once(&store, &mut credentials).await {
            Ok(()) => {
                wait = FIRST;
                FIRST
            }
            Err(e) => {
                println!("{e}");
                let delay = wait;
                wait = (wait * 2).min(MAX);
                delay
            }
        };
        println!("reconnecting in {} s (Ctrl+C quits)", delay.as_secs());
        tokio::time::sleep(delay).await;
    }
}
