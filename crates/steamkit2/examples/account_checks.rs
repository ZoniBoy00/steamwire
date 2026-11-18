//! Live checks that need a real account. Logs on with a QR code (or, with
//! `--credentials`, a user name, password and Steam Guard), then calls the
//! handler APIs an anonymous user can't reach. `--code` (with
//! `--credentials`) declines the mobile app confirmation, so the Steam Guard
//! code is asked for instead, as SteamKit2 falls back to the next method. Each line is `ok`, `steam`
//! (Steam answered with an error, so the request and reply were well formed)
//! or `FAIL`.
//!
//! By default every call only reads. `--write` also makes changes that are
//! undone at the end: a private Spacewar lobby that is left again, and the
//! persona state, which is set back. Nothing else on the account changes.
//!
//! Tokens, the email address and the wallet are never printed or stored.
//!
//! ```sh
//! cargo run -p steamkit2 --example account_checks [-- --write] [-- --credentials [--code]]
//! ```

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qrcode::render::unicode;
use qrcode::QrCode;
use steamkit2::enums::{EAccountType, ELeaderboardDataRequest, ELobbyType, EPersonaState, EWorkshopFileAction};
use steamkit2::handlers::auth::{AuthSessionDetails, AuthTokens, Authenticator, UserConsoleAuthenticator};
use steamkit2::handlers::matchmaking::Metadata;
use steamkit2::handlers::user::LogOnDetails;
use steamkit2::handlers::Callback;
use steamkit2::types::UgcHandle;
use steamkit2::{Config, Error, SteamClient, SteamId};

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn check<T: std::fmt::Debug>(&mut self, name: &str, result: steamkit2::Result<T>) -> Option<T> {
        match result {
            Ok(value) => {
                let shown = format!("{value:?}");
                println!("ok    {name}: {}", shown.chars().take(100).collect::<String>());
                Some(value)
            }
            Err(e @ (Error::Steam(_) | Error::SteamMessage { .. })) => {
                println!("steam {name}: {e}");
                None
            }
            Err(e) => {
                println!("FAIL  {name}: {e}");
                self.failures += 1;
                None
            }
        }
    }

    fn expect(&mut self, name: &str, passed: bool, detail: impl std::fmt::Display) {
        println!("{} {name}: {detail}", if passed { "ok   " } else { "FAIL " });
        if !passed {
            self.failures += 1;
        }
    }
}

/// Counts the callbacks that arrive, by name (`Friends::FriendsList`, ...).
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<BTreeMap<String, usize>>>);

impl Seen {
    fn record(&self, callback: &Callback) {
        let shown = format!("{callback:?}");
        let mut parts = shown.split('(');
        let outer = parts.next().unwrap_or("");
        let inner = parts.next().unwrap_or("").split([' ', '{']).next().unwrap_or("");
        let name = format!("{outer}::{inner}");
        *self.0.lock().unwrap().entry(name).or_default() += 1;
    }

    fn count(&self, name: &str) -> usize {
        self.0.lock().unwrap().get(name).copied().unwrap_or(0)
    }

    /// Waits up to `limit` for a callback.
    async fn wait_for(&self, name: &str, limit: Duration) -> bool {
        let start = self.count(name);
        let deadline = tokio::time::Instant::now() + limit;
        while tokio::time::Instant::now() < deadline {
            if self.count(name) > start {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    fn has(&self, name: &str) -> bool {
        self.count(name) > 0
    }
}

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

/// Asks for codes on the console, but declines the mobile app confirmation.
struct CodeOnly;

impl Authenticator for CodeOnly {
    async fn device_code(&mut self, previous_code_was_incorrect: bool) -> steamkit2::Result<String> {
        UserConsoleAuthenticator.device_code(previous_code_was_incorrect).await
    }

    async fn email_code(&mut self, email: &str, previous_code_was_incorrect: bool) -> steamkit2::Result<String> {
        UserConsoleAuthenticator.email_code(email, previous_code_was_incorrect).await
    }

    async fn accept_device_confirmation(&mut self) -> bool {
        false
    }
}

fn prompt(label: &str) -> String {
    print!("{label}: ");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    line.trim().to_string()
}

async fn authenticate(
    client: &SteamClient,
    report: &mut Report,
    credentials: bool,
    code_only: bool,
) -> steamkit2::Result<AuthTokens> {
    let details = AuthSessionDetails::default();
    if credentials {
        let username = prompt("Steam user name");
        let password = rpassword::prompt_password("password (hidden): ")?;
        let mut session = client.begin_auth_session_via_credentials(&username, &password, &details).await?;
        report.expect(
            "credentials session",
            session.client_id() != 0 && !session.request_id().is_empty(),
            format!("allowed confirmations {:?}", session.allowed_confirmations()),
        );
        // Asks for the email or authenticator code (again if it's wrong), or
        // waits for the confirmation in the mobile app.
        if code_only {
            println!("Declining the app confirmation: enter a wrong code first to check that it asks again.");
            session.wait_with(&mut CodeOnly).await
        } else {
            session.wait_with(&mut UserConsoleAuthenticator).await
        }
    } else {
        let mut session = client.begin_auth_session_via_qr(&details).await?;
        report.expect(
            "QR session",
            session.client_id() != 0 && !session.request_id().is_empty() && !session.interval().is_zero(),
            format!("poll interval {:?}", session.interval()),
        );
        print_qr(session.challenge_url());
        println!("Scan the code with the Steam mobile app and approve the sign-in.");
        session
            .wait(|url| {
                println!("The QR code changed:");
                print_qr(url);
            })
            .await
    }
}

async fn connect(seen: &Seen) -> steamkit2::Result<SteamClient> {
    let (client, mut events) = SteamClient::connect(Config::default()).await?;
    let seen = seen.clone();
    tokio::spawn(async move {
        while let Some(packet) = events.recv_packet().await {
            if let Some(callback) = Callback::from_packet(&packet) {
                seen.record(&callback);
            }
        }
    });
    Ok(client)
}

#[tokio::main]
async fn main() -> steamkit2::Result<()> {
    let write = std::env::args().any(|a| a == "--write");
    let credentials = std::env::args().any(|a| a == "--credentials");
    let code_only = std::env::args().any(|a| a == "--code");
    let mut report = Report::default();
    let seen = Seen::default();

    // --- Authentication and logon.
    let client = connect(&seen).await?;
    let tokens = match authenticate(&client, &mut report, credentials, code_only).await {
        Ok(tokens) => tokens,
        Err(e) => {
            println!("FAIL  authentication: {e}");
            std::process::exit(1);
        }
    };
    report.expect("tokens", !tokens.refresh_token.is_empty() && !tokens.access_token.is_empty(), "received");

    let logon = LogOnDetails {
        username: tokens.account_name.clone(),
        access_token: Some(tokens.refresh_token.clone()),
        ..LogOnDetails::default()
    };
    if report.check("log_on with the refresh token", client.log_on(logon.clone()).await.map(|l| l.steam_id)).is_none() {
        std::process::exit(1);
    }
    let me = client.steam_id().unwrap();
    report.expect("individual SteamID", me.account_type() == EAccountType::Individual, me.steam3());

    for name in ["Apps::LicenseList", "User::AccountInfo", "Friends::FriendsList"] {
        let arrived = seen.has(name) || seen.wait_for(name, Duration::from_secs(10)).await;
        report.expect(&format!("{name} after logon"), arrived, "");
    }

    report.check(
        "generate_access_token",
        client
            .generate_access_token(me, &tokens.refresh_token, false)
            .await
            .map(|r| r.access_token.as_deref().is_some_and(|t| !t.is_empty())),
    );

    // --- Friends (cached state and read-only requests).
    report.expect("persona_name", client.persona_name().is_some_and(|n| !n.is_empty()), "set");
    println!("      persona state {:?}", client.persona_state());
    let friends = client.friends();
    println!("      {} friends, {} groups", friends.len(), client.clans().len());
    if let Some(&first) = friends.first() {
        let friend = client.friend(first);
        report.expect(
            "friend() cache",
            friend.as_ref().is_some_and(|f| f.name.is_some()),
            format!("name known, avatar URL {}", friend.and_then(|f| f.avatar_url()).is_some()),
        );

        report.check("request_message_history", client.request_message_history(first));
        let arrived = seen.wait_for("Friends::MessageHistory", Duration::from_secs(10)).await;
        report.expect("MessageHistory reply", arrived, "");
    } else {
        println!("skip  friend requests: the account has no friends");
    }
    // Friends' persona states were requested on logon (as SteamKit2 does), and
    // Steam doesn't send unchanged ones again, so ask for someone else.
    let stranger = SteamId::from_u64(76561197960287930);
    if !friends.contains(&stranger) {
        report.check("request_friend_info (not a friend)", client.request_friend_info(&[stranger], None));
        let arrived = seen.wait_for("Friends::PersonaState", Duration::from_secs(10)).await;
        report.expect("PersonaState reply", arrived, "");
    }
    if let Some(&clan) = client.clans().first() {
        report.expect("clan() cache", client.clan(clan).is_some(), "");
    }
    report.check("request_offline_messages", client.request_offline_messages());
    report.check("request_profile_info (self)", client.request_profile_info(me).await.map(|p| p.eresult()));

    // --- Apps, tickets and content.
    report.check(
        "pics_get_access_tokens",
        client.pics_get_access_tokens(&[480, 7], &[]).await.map(|r| r.app_access_tokens.len()),
    );
    report.check("get_app_ownership_ticket 7", client.get_app_ownership_ticket(7).await.map(|r| r.ticket().len()));
    report.check(
        "check_app_beta_password (wrong)",
        client.check_app_beta_password(480, "steamkit-test").await.map(|r| r.betapasswords.len()),
    );
    report.check("get_legacy_game_key 480", client.get_legacy_game_key(480).await.map(|k| k.len()));
    report.check("get_auth_session_ticket 480", client.get_auth_session_ticket(480).await.map(|t| t.ticket.len()));
    report.check(
        "get_auth_ticket_for_web_api 480",
        client.get_auth_ticket_for_web_api(480, "steamkit").await.map(|t| t.ticket.len()),
    );
    if let Some(server) = client.get_servers_for_steam_pipe(None, Some(1)).await?.into_iter().next() {
        report
            .check("get_cdn_auth_token", client.get_cdn_auth_token(480, 481, &server.host).await.map(|t| t.expiration));
    }

    // --- Stats, matchmaking, networking, workshop, cloud.
    report.check("get_number_of_current_players 730", client.get_number_of_current_players(730).await);
    if let Some(board) = report.check("find_leaderboard", client.find_leaderboard(480, "Feet Traveled").await) {
        report.check(
            "get_leaderboard_entries",
            client.get_leaderboard_entries(480, board.id, 1, 5, ELeaderboardDataRequest::Global).await.map(|e| e.len()),
        );
        report.check(
            "download_leaderboard_entries_for_users",
            client.download_leaderboard_entries_for_users(480, board.id, &[me]).await.map(|e| e.len()),
        );
    }
    report.check("get_lobby_list 480", client.get_lobby_list(480, &[], 10).await.map(|l| l.len()));
    report.check(
        "request_networking_certificate",
        client.request_networking_certificate(480, &[7u8; 32]).await.map(|r| r.cert.is_some()),
    );
    report.check(
        "enumerate_published_files_by_user_action",
        client
            .enumerate_published_files_by_user_action(440, EWorkshopFileAction::Played, 0)
            .await
            .map(|r| r.total_results()),
    );
    report.check("get_single_file_info (missing file)", client.get_single_file_info(480, "steamkit-missing.txt").await);
    report.check("request_ugc_details (missing)", client.request_ugc_details(UgcHandle::from(1u64)).await.map(|_| ()));

    // --- Changes that are undone.
    if write {
        let original = client.persona_state();
        let other = if original == EPersonaState::Away { EPersonaState::Online } else { EPersonaState::Away };
        report.check("set_persona_state", client.set_persona_state(other));
        tokio::time::sleep(Duration::from_secs(2)).await;
        report.check("set_persona_state (back)", client.set_persona_state(original));

        let metadata = Metadata::from([("steamkit".to_string(), "test".to_string())]);
        if let Some(lobby) = report.check(
            "create_lobby (private, Spacewar)",
            client.create_lobby(480, ELobbyType::Private, 2, 0, &metadata).await,
        ) {
            let more = Metadata::from([("steamkit".to_string(), "changed".to_string())]);
            report.check("set_lobby_data", client.set_lobby_data(480, lobby, ELobbyType::Private, 2, 0, &more).await);
            report.check("set_lobby_member_data", client.set_lobby_member_data(480, lobby, &metadata).await);
            report.check("get_lobby_data", client.get_lobby_data(480, lobby).await.map(|l| l.metadata));
            report.expect("lobby() cache", client.lobby(480, lobby).is_some(), "");
            report.check("leave_lobby", client.leave_lobby(480, lobby).await);
        }
    }

    // --- Token logon on a second connection (what a saved token does).
    client.log_off()?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let again = connect(&seen).await?;
    report.check("second log_on with the same token", again.log_on(logon).await.map(|l| l.steam_id == me));
    again.log_off()?;

    println!("\ncallbacks seen: {:?}", seen.0.lock().unwrap().keys().collect::<Vec<_>>());
    println!("{} failure(s)", report.failures);
    std::process::exit(if report.failures == 0 { 0 } else { 1 });
}
