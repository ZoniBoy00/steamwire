//! Logs on to a real account with a QR code: scan it with the Steam mobile app.
//!
//! ```sh
//! cargo run -p steamkit --example qr_login
//! ```
//!
//! With `-- --dry-run` it only starts the session and prints the challenge
//! URL, which checks the flow without an account.

use qrcode::render::unicode;
use qrcode::QrCode;
use steamkit::handlers::auth::AuthSessionDetails;
use steamkit::handlers::user::LogOnDetails;
use steamkit::{Config, EPersonaState, SteamClient};

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

#[tokio::main]
async fn main() -> steamkit::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let dry_run = std::env::args().any(|a| a == "--dry-run");

    let (client, _events) = SteamClient::connect(Config::default()).await?;

    let mut session = client.begin_auth_session_via_qr(&AuthSessionDetails::default()).await?;
    println!("challenge URL: {}", session.challenge_url());
    if dry_run {
        return Ok(());
    }
    print_qr(session.challenge_url());
    println!("Scan the code with the Steam mobile app.");

    let tokens = session
        .wait(|url| {
            println!("The QR code changed:");
            print_qr(url);
        })
        .await?;
    println!("authenticated as {}", tokens.account_name);

    let logged_on = client
        .log_on(LogOnDetails {
            username: tokens.account_name.clone(),
            access_token: Some(tokens.refresh_token.clone()),
            ..LogOnDetails::default()
        })
        .await?;
    println!("logged on as {}", logged_on.steam_id);

    client.set_persona_state(EPersonaState::Online)?;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    client.log_off()?;
    Ok(())
}
