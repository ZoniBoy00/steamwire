//! Live checks that change accounts, with two of your own accounts: A (the
//! main one) and B (a test friend). Run `account_checks` first.
//!
//! Between A and B (undone at the end): a friend request by account name and
//! its acceptance, a chat message, a persona name change seen by the friend
//! (and changed back), a friends-only Spacewar lobby with an invite, a join
//! and an owner change, blocking and unblocking, and removing the friend
//! again. If A and B were already friends, the friendship is kept and the
//! request, block and removal are skipped; with `--refriend` the friend is
//! removed first and added back, so they run and the friendship stays.
//!
//! On account A only, and left behind: a Spacewar leaderboard named
//! "steamkit test", a private Spacewar screenshot entry (delete it from your
//! profile's screenshots), and Team Fortress 2 in the library (a free
//! license) so its game coordinator can be greeted; A shows as playing TF2
//! for a few seconds. The temporary persona name stays in A's name history.
//!
//! ```sh
//! cargo run -p steamkit --example account_changes [-- --refriend]
//! ```

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use qrcode::render::unicode;
use qrcode::QrCode;
use steamkit::enums::{
    EChatEntryType, EFriendRelationship, ELeaderboardDisplayType, ELeaderboardSortMethod, ELobbyType, EPersonaState,
    EUCMFilePrivacyState,
};
use steamkit::handlers::auth::{AuthSessionDetails, UserConsoleAuthenticator};
use steamkit::handlers::matchmaking::Metadata;
use steamkit::handlers::screenshots::ScreenshotDetails;
use steamkit::handlers::user::LogOnDetails;
use steamkit::handlers::Callback;
use steamkit::messages::gc::GcPacket;
use steamkit::protos::steam::{c_msg_client_games_played::GamePlayed, CMsgClientGamesPlayed};
use steamkit::{Config, EMsg, Error, GameId, ProtoMsg, SteamClient, SteamId};

/// TF2's GC: `k_EMsgGCClientWelcome` and `k_EMsgGCClientHello`.
const GC_CLIENT_WELCOME: u32 = 4004;
const GC_CLIENT_HELLO: u32 = 4006;

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn check<T: std::fmt::Debug>(&mut self, name: &str, result: steamkit::Result<T>) -> Option<T> {
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

    fn expect(&mut self, name: &str, passed: bool, detail: impl std::fmt::Display) -> bool {
        println!("{} {name}: {detail}", if passed { "ok   " } else { "FAIL " });
        if !passed {
            self.failures += 1;
        }
        passed
    }
}

/// Counts the callbacks (and unhandled messages) a client receives.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<BTreeMap<String, (usize, String)>>>);

impl Seen {
    fn record(&self, name: String, detail: String) {
        let mut seen = self.0.lock().unwrap();
        let entry = seen.entry(name).or_default();
        entry.0 += 1;
        entry.1 = detail;
    }

    fn count(&self, name: &str) -> usize {
        self.0.lock().unwrap().get(name).map(|e| e.0).unwrap_or(0)
    }

    /// The last one received, as `Debug` output.
    fn last(&self, name: &str) -> String {
        self.0.lock().unwrap().get(name).map(|e| e.1.clone()).unwrap_or_default()
    }

    /// Waits up to `limit` for one more of a callback than `since`.
    async fn wait_past(&self, name: &str, since: usize, limit: Duration) -> bool {
        until(limit, || self.count(name) > since).await
    }
}

/// Polls `condition` every 200 ms for up to `limit`.
async fn until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if condition() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn callback_name(callback: &Callback) -> String {
    if let Callback::GameCoordinator(message) = callback {
        return format!("GC::{}", message.msg_type());
    }
    let shown = format!("{callback:?}");
    let mut parts = shown.split('(');
    let outer = parts.next().unwrap_or("");
    let inner = parts.next().unwrap_or("").split([' ', '{']).next().unwrap_or("");
    format!("{outer}::{inner}")
}

fn print_qr(url: &str) {
    match QrCode::new(url) {
        Ok(code) => println!("{}", code.render::<unicode::Dense1x2>().quiet_zone(true).build()),
        Err(_) => println!("{url}"),
    }
}

/// Connects and logs on, by QR code or by user name and password.
async fn log_on(label: &str, report: &mut Report) -> steamkit::Result<(SteamClient, Seen, String)> {
    let (client, mut events) = SteamClient::connect(Config::default()).await?;
    let seen = Seen::default();
    let recorder = seen.clone();
    tokio::spawn(async move {
        while let Some(packet) = events.recv_packet().await {
            match Callback::from_packet(&packet) {
                Some(callback) => recorder.record(callback_name(&callback), format!("{callback:?}")),
                None => recorder.record(format!("{:?}", packet.emsg), String::new()),
            }
        }
    });

    print!("\n{label}: press Enter for a QR code, or type a user name to use a password: ");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    let username = line.trim().to_string();
    let details = AuthSessionDetails::default();
    let tokens = if username.is_empty() {
        let mut session = client.begin_auth_session_via_qr(&details).await?;
        print_qr(session.challenge_url());
        println!("Scan it with the Steam mobile app signed in to {label}.");
        session
            .wait(|url| {
                println!("The QR code changed:");
                print_qr(url);
            })
            .await?
    } else {
        let password = rpassword::prompt_password("password (hidden): ")?;
        let mut session = client.begin_auth_session_via_credentials(&username, &password, &details).await?;
        session.wait_with(&mut UserConsoleAuthenticator).await?
    };
    let logged_on = client
        .log_on(LogOnDetails {
            username: tokens.account_name.clone(),
            access_token: Some(tokens.refresh_token.clone()),
            ..LogOnDetails::default()
        })
        .await?;
    report.expect(&format!("{label} logged on"), true, logged_on.steam_id.steam3());
    // The friends list and persona states arrive right after logon.
    until(Duration::from_secs(10), || seen.count("Friends::FriendsList") > 0).await;
    Ok((client, seen, tokens.account_name))
}

fn relationship(client: &SteamClient, other: SteamId) -> EFriendRelationship {
    client.friend(other).map(|f| f.relationship).unwrap_or(EFriendRelationship::None)
}

#[tokio::main]
async fn main() -> steamkit::Result<()> {
    let mut report = Report::default();
    let (a, seen_a, _) = log_on("account A (main)", &mut report).await?;
    let (b, seen_b, name_b) = log_on("account B (test friend)", &mut report).await?;
    let (id_a, id_b) = (a.steam_id().unwrap(), b.steam_id().unwrap());
    if id_a == id_b {
        println!("A and B are the same account");
        std::process::exit(1);
    }
    println!();

    // Online, so the friend sees persona changes; set back at the end.
    let (state_a, state_b) = (a.persona_state(), b.persona_state());
    report.check("A set_persona_state Online", a.set_persona_state(EPersonaState::Online));
    report.check("B set_persona_state Online", b.set_persona_state(EPersonaState::Online));

    // --- Friend request by account name, accepted by Steam ID.
    let already_friends = relationship(&a, id_b) == EFriendRelationship::Friend;
    let refriend = std::env::args().any(|arg| arg == "--refriend");
    if already_friends && refriend {
        report.check("A remove_friend(B) (to add again)", a.remove_friend(id_b));
        let gone = until(Duration::from_secs(15), || relationship(&b, id_a) == EFriendRelationship::None).await;
        report.expect("B sees the removal", gone, format!("{:?}", relationship(&b, id_a)));
    }
    let test_requests = !already_friends || refriend;
    if test_requests {
        let added = seen_a.count("Friends::FriendAdded");
        report.check("A add_friend_by_name(B)", a.add_friend_by_name(&name_b));
        if seen_a.wait_past("Friends::FriendAdded", added, Duration::from_secs(10)).await {
            println!(
                "      Steam's answer: {}",
                seen_a.last("Friends::FriendAdded").chars().take(150).collect::<String>()
            );
        }
        let by_name =
            until(Duration::from_secs(10), || relationship(&b, id_a) == EFriendRelationship::RequestRecipient).await;
        if !by_name {
            println!("      the request by account name did not arrive; sending it by Steam ID");
            report.check("A add_friend(B)", a.add_friend(id_b));
        }
        let seen =
            until(Duration::from_secs(15), || relationship(&b, id_a) == EFriendRelationship::RequestRecipient).await;
        report.expect("B sees the request", seen, format!("{:?}", relationship(&b, id_a)));
        report.check("B add_friend(A) (accept)", b.add_friend(id_a));
        let friends = until(Duration::from_secs(15), || relationship(&a, id_b) == EFriendRelationship::Friend).await;
        report.expect("A and B are friends", friends, format!("{:?}", relationship(&a, id_b)));
    } else {
        println!("skip  friend request, block and removal: A and B are already friends (see --refriend)");
    }

    // --- Chat message.
    let before = seen_b.count("Friends::FriendMsg");
    report.check("A send_chat_message(B)", a.send_chat_message(id_b, EChatEntryType::ChatMsg, "steamkit test message"));
    let arrived = seen_b.wait_past("Friends::FriendMsg", before, Duration::from_secs(15)).await;
    report.expect("B receives it", arrived, "");

    // --- Persona name, as the friend sees it. Steam may censor parts of the
    // new name ("steam" becomes "*****"), so any change counts.
    if let Some(original) = a.persona_name() {
        let name_at_b = || b.friend(id_a).and_then(|f| f.name);
        report.check("A set_persona_name (temporary)", a.set_persona_name(format!("{original} [steamkit]")));
        let changed = until(Duration::from_secs(20), || name_at_b().is_some_and(|n| n != original)).await;
        report.expect("B sees the new name", changed, format!("{:?}", name_at_b()));
        report.check("A set_persona_name (back)", a.set_persona_name(original.as_str()));
        let back = until(Duration::from_secs(20), || name_at_b().as_deref() == Some(&original)).await;
        report.expect("B sees the original name", back, format!("{:?}", name_at_b()));
    }

    // --- Lobby: invite, join, owner change.
    let metadata = Metadata::from([("steamkit".to_string(), "two accounts".to_string())]);
    if let Some(lobby) = report
        .check("A create_lobby (friends only)", a.create_lobby(480, ELobbyType::FriendsOnly, 2, 0, &metadata).await)
    {
        // Lobby invites arrive as chat invites (ClientChatInvite).
        let invites = seen_b.count("Friends::ChatInvite");
        report.check("A invite_to_lobby(B)", a.invite_to_lobby(480, lobby, id_b));
        let invited = seen_b.wait_past("Friends::ChatInvite", invites, Duration::from_secs(10)).await;
        report.expect("B receives the invite", invited, "");

        let joins = seen_a.count("Matchmaking::UserJoinedLobby");
        report.check("B join_lobby", b.join_lobby(480, lobby).await.map(|j| j.response));
        let joined = seen_a.wait_past("Matchmaking::UserJoinedLobby", joins, Duration::from_secs(10)).await;
        report.expect("A sees B join", joined, "");

        report.check("A set_lobby_owner(B)", a.set_lobby_owner(480, lobby, id_b).await);
        let owner = b.get_lobby_data(480, lobby).await.map(|l| l.owner);
        report.expect("B is the owner", matches!(owner, Ok(Some(o)) if o == id_b), format!("{owner:?}"));
        report.check("B leave_lobby", b.leave_lobby(480, lobby).await);
        report.check("A leave_lobby", a.leave_lobby(480, lobby).await);
    }

    // --- Block and unblock; remove the friend again unless it was one before.
    if test_requests {
        report.check("A ignore_friend(B, true)", a.ignore_friend(id_b, true).await);
        let is_blocked = || {
            matches!(
                relationship(&a, id_b),
                EFriendRelationship::IgnoredFriend | EFriendRelationship::Ignored | EFriendRelationship::Blocked
            )
        };
        let blocked = until(Duration::from_secs(10), is_blocked).await;
        report.expect("B is blocked", blocked, format!("{:?}", relationship(&a, id_b)));
        report.check("A ignore_friend(B, false)", a.ignore_friend(id_b, false).await);
        let unblocked = until(Duration::from_secs(10), || !is_blocked()).await;
        report.expect("B is unblocked", unblocked, format!("{:?}", relationship(&a, id_b)));
        if already_friends {
            if relationship(&a, id_b) != EFriendRelationship::Friend {
                report.check("B add_friend(A) (restore)", b.add_friend(id_a));
                report.check("A add_friend(B) (restore)", a.add_friend(id_b));
            }
            let friends =
                until(Duration::from_secs(15), || relationship(&a, id_b) == EFriendRelationship::Friend).await;
            report.expect("still friends at the end", friends, format!("{:?}", relationship(&a, id_b)));
        } else {
            if relationship(&a, id_b) != EFriendRelationship::None {
                report.check("A remove_friend(B)", a.remove_friend(id_b));
            }
            let gone = until(Duration::from_secs(15), || relationship(&a, id_b) == EFriendRelationship::None).await;
            report.expect("A and B are no longer friends", gone, format!("{:?}", relationship(&a, id_b)));
        }
    }

    report.check("B set_persona_state (back)", b.set_persona_state(state_b));
    b.log_off()?;

    // --- Account A only.
    let board = report.check(
        "create_leaderboard (Spacewar, \"steamkit test\")",
        a.create_leaderboard(
            480,
            "steamkit test",
            ELeaderboardSortMethod::Descending,
            ELeaderboardDisplayType::Numeric,
        )
        .await,
    );
    if let Some(board) = board {
        let found = a.find_leaderboard(480, "steamkit test").await;
        report.expect("find_leaderboard finds it", matches!(&found, Ok(f) if f.id == board.id), format!("{found:?}"));
    }

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as u32;
    report.check(
        "add_screenshot (private, Spacewar)",
        a.add_screenshot(&ScreenshotDetails {
            game_id: Some(GameId::from_app_id(480)),
            ufs_image_file_path: "screenshots/steamkit_test.jpg".into(),
            ufs_thumbnail_file_path: "screenshots/thumbnails/steamkit_test.jpg".into(),
            caption: "steamkit test".into(),
            privacy: EUCMFilePrivacyState::Private,
            width: 640,
            height: 480,
            creation_time: now,
            contains_spoilers: false,
        })
        .await,
    );

    // --- TF2's game coordinator: play the game, say hello, expect a welcome.
    report.check("request_free_license TF2", a.request_free_license(&[440]).await.map(|r| r.granted_appids));
    let games = |ids: &[u64]| {
        ProtoMsg::new(
            EMsg::ClientGamesPlayed,
            CMsgClientGamesPlayed {
                games_played: ids.iter().map(|&id| GamePlayed { game_id: Some(id), ..Default::default() }).collect(),
                ..Default::default()
            },
        )
    };
    report.check("games played: TF2", a.send(games(&[440])));
    let welcomes = seen_a.count(&format!("GC::{GC_CLIENT_WELCOME}"));
    let mut welcomed = false;
    for _ in 0..6 {
        report.check("send_gc ClientHello", a.send_gc(440, &GcPacket::proto(GC_CLIENT_HELLO, &())));
        if seen_a.wait_past(&format!("GC::{GC_CLIENT_WELCOME}"), welcomes, Duration::from_secs(5)).await {
            welcomed = true;
            break;
        }
    }
    report.expect("TF2 GC ClientWelcome", welcomed, "");
    report.check("games played: none", a.send(games(&[])));

    report.check("A set_persona_state (back)", a.set_persona_state(state_a));
    tokio::time::sleep(Duration::from_secs(1)).await;
    a.log_off()?;

    println!("\nA saw: {:?}", seen_a.0.lock().unwrap().keys().collect::<Vec<_>>());
    println!("B saw: {:?}", seen_b.0.lock().unwrap().keys().collect::<Vec<_>>());
    println!("{} failure(s)", report.failures);
    std::process::exit(if report.failures == 0 { 0 } else { 1 });
}
