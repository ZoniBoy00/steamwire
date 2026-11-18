//! Live checks of the APIs an anonymous account or game server can reach,
//! beyond `anonymous` and `handlers_tour`. Each line is `ok` (the expected
//! answer), `steam` (Steam answered with an error, so the request and reply
//! were well formed), `anon` (no answer, as SteamKit2 also gets for an
//! anonymous user; `account_checks` covers these) or `FAIL`.
//!
//! ```sh
//! cargo run -p steamwire --example live_checks
//! ```

use std::time::{Duration, Instant};

use steamwire::enums::{EOSType, EServerFlags};
use steamwire::handlers::game_server::StatusDetails;
use steamwire::handlers::user::AnonymousLogOnDetails;
use steamwire::protos::steam::CPlayerGetGameBadgeLevelsRequest;
use steamwire::webapi::WebApi;
use steamwire::{CmServer, Config, Error, SteamClient};

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn check<T: std::fmt::Debug>(&mut self, name: &str, result: steamwire::Result<T>) -> Option<T> {
        match result {
            Ok(value) => {
                let shown = format!("{value:?}");
                println!("ok    {name}: {}", shown.chars().take(110).collect::<String>());
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

    /// A call Steam ignores from anonymous users (checked against SteamKit2):
    /// a timeout is expected, any reply is fine too.
    fn anonymous<T: std::fmt::Debug>(&mut self, name: &str, result: steamwire::Result<T>) {
        match result {
            Err(Error::Timeout) => println!("anon  {name}: no answer for anonymous users"),
            other => {
                self.check(name, other);
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

#[tokio::main]
async fn main() -> steamwire::Result<()> {
    let mut report = Report::default();

    // Web API options.
    let fast = WebApi::new().with_timeout(Duration::from_millis(1));
    let result = fast.content_servers(0, None).await;
    report.expect("WebApi::with_timeout(1 ms)", result.is_err(), format!("{:?}", result.err()));
    let custom =
        WebApi::new().with_http_client(reqwest::Client::builder().user_agent("steamwire-live-checks").build()?);
    report.check("WebApi::with_http_client", custom.content_servers(0, Some(2)).await.map(|s| s.len()));

    // A server that never answers: connecting must give up after
    // connection_timeout.
    let started = Instant::now();
    let config = Config {
        server: Some(CmServer::tcp("10.255.255.1:27017")),
        connection_timeout: Duration::from_secs(1),
        connect_attempts: 1,
        ..Config::default()
    };
    let result = SteamClient::connect(config).await.map(|_| ());
    let took = started.elapsed();
    report.expect(
        "Config::connection_timeout (1 s)",
        result.is_err() && took < Duration::from_secs(3),
        format!("{:?} after {took:.1?}", result.err()),
    );

    // Anonymous user.
    let config = Config { job_timeout: Duration::from_secs(3), ..Config::default() };
    let server_list = config.server_list.clone();
    let (client, _events) = SteamClient::connect(config).await?;
    let before = server_list.last_refresh();
    report.check("ServerList::force_refresh", server_list.force_refresh().await.map(|_| server_list.servers().len()));
    report.expect(
        "ServerList::last_refresh",
        server_list.last_refresh().is_some() && server_list.last_refresh() != before,
        format!("{before:?} -> {:?}", server_list.last_refresh()),
    );

    let details =
        AnonymousLogOnDetails { cell_id: Some(0), client_os_type: EOSType::Win11, client_language: "finnish".into() };
    let logged_on = report.check("log_on_anonymous_with", client.log_on_anonymous_with(details).await);
    if logged_on.is_none() {
        println!("cannot go on without a logon");
        std::process::exit(1);
    }
    report.expect(
        "anonymous SteamID",
        client.steam_id().is_some_and(|id| id.is_anon_user_account()),
        format!("{:?}", client.steam_id()),
    );
    report.expect(
        "public IP and country",
        client.public_ip().is_some() && client.ip_country_code().is_some(),
        format!("country {:?}, local IP known: {}", client.ip_country_code(), client.local_ip().is_some()),
    );

    report.check("pics_get_access_tokens", client.pics_get_access_tokens(&[480], &[0]).await);

    let host = client.get_servers_for_steam_pipe(None, Some(1)).await?.first().map(|s| s.host.clone());
    if let Some(host) = host {
        report.check(&format!("get_cdn_auth_token ({host})"), client.get_cdn_auth_token(480, 481, &host).await);
    }

    report.anonymous("request_networking_certificate", client.request_networking_certificate(480, &[7u8; 32]).await);

    report.anonymous("get_lobby_list 480", client.get_lobby_list(480, &[], 10).await.map(|lobbies| lobbies.len()));

    report.anonymous("find_leaderboard", client.find_leaderboard(480, "Feet Traveled").await);

    // A message with no reply for anonymous accounts: the job must give up
    // after job_timeout (3 s here).
    let started = Instant::now();
    let result = client.get_number_of_current_players(730).await;
    let took = started.elapsed();
    match result {
        Err(Error::Timeout) => report.expect(
            "Config::job_timeout (3 s)",
            took < Duration::from_secs(5),
            format!("timed out after {took:.1?}"),
        ),
        other => println!("skip  Config::job_timeout: Steam answered ({other:?})"),
    }

    // A notification has no reply; the connection must stay usable after it.
    report.check(
        "send_service_notification",
        client.send_service_notification(
            "Player.GetGameBadgeLevels#1",
            CPlayerGetGameBadgeLevelsRequest { appid: Some(440) },
        ),
    );
    report.check("still connected after it", client.pics_get_access_tokens(&[440], &[]).await.map(|_| ()));
    client.log_off()?;

    // Anonymous game server.
    let (server, mut events) = SteamClient::connect(Config::default()).await?;
    if report.check("log_on_game_server_anonymous", server.log_on_game_server_anonymous(480).await).is_some() {
        report.expect(
            "game server SteamID",
            server.steam_id().is_some_and(|id| id.is_anon_game_server_account()),
            format!("{:?}", server.steam_id()),
        );
        let status = StatusDetails {
            app_id: 480,
            server_flags: EServerFlags::Dedicated,
            game_directory: "spacewar".into(),
            address: None,
            port: 27015,
            query_port: 27016,
            version: "1.0.0.0".into(),
        };
        report.check("send_game_server_status", server.send_game_server_status(&status));
        tokio::time::sleep(Duration::from_secs(2)).await;
        report.check("log_off_game_server", server.log_off_game_server());
        let disconnected = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = events.recv().await {
                if matches!(event, steamwire::Event::Disconnected { .. }) {
                    return true;
                }
            }
            true
        })
        .await
        .unwrap_or(false);
        report.expect("disconnected after log off", disconnected, "");
    }

    println!("\n{} failure(s)", report.failures);
    std::process::exit(if report.failures == 0 { 0 } else { 1 });
}
