//! Logs on anonymously and calls a handful of handler APIs, printing what
//! each returns. Some calls (player counts, leaderboards, master server)
//! get no answer for anonymous accounts and time out after 10 seconds.
//!
//! ```sh
//! cargo run -p steamwire --example handlers_tour
//! ```

use std::time::Duration;

use steamwire::enums::ERegionCode;
use steamwire::handlers::master_server::QueryDetails;
use steamwire::handlers::Callback;
use steamwire::{Config, SteamClient};

/// Runs a call with a 10 second limit.
async fn quick<T>(f: impl std::future::Future<Output = steamwire::Result<T>>) -> steamwire::Result<T> {
    tokio::time::timeout(Duration::from_secs(10), f).await.unwrap_or(Err(steamwire::Error::Timeout))
}

#[tokio::main]
async fn main() -> steamwire::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let (client, mut events) = SteamClient::connect(Config::default()).await?;
    client.log_on_anonymous().await?;
    println!("logged on as {}", client.steam_id().unwrap());

    match quick(client.get_number_of_current_players(730)).await {
        Ok(n) => println!("current players in 730: {n}"),
        Err(e) => println!("current players: {e}"),
    }

    match quick(client.pics_get_changes_since(0, true, false)).await {
        Ok(c) => println!(
            "PICS: current change number {}, full update needed: {}",
            c.current_change_number(),
            c.force_full_update()
        ),
        Err(e) => println!("PICS changes: {e}"),
    }

    match quick(client.get_servers_for_steam_pipe(None, Some(5))).await {
        Ok(servers) => {
            for s in &servers {
                println!("content server {s} load {}", s.weighted_load);
            }
        }
        Err(e) => println!("content servers: {e}"),
    }

    let query = QueryDetails {
        app_id: 440,
        filter: r"\appid\440\empty\1".into(),
        region: ERegionCode::World,
        geo_located_ip: None,
        max_servers: 5,
    };
    match quick(client.server_query(&query)).await {
        Ok(servers) => println!("master server: {} servers, first {:?}", servers.len(), servers.first()),
        Err(e) => println!("master server: {e}"),
    }

    match quick(client.find_leaderboard(480, "Feet Traveled")).await {
        Ok(lb) => println!("leaderboard: {lb:?}"),
        Err(e) => println!("leaderboard: {e}"),
    }

    // Show what arrived unasked (license list, ...), decoded as callbacks.
    let drain = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(packet) = events.recv_packet().await {
            match Callback::from_packet(&packet) {
                Some(cb) => println!("callback: {}", format!("{cb:?}").chars().take(120).collect::<String>()),
                None => println!("unhandled message {:?}", packet.emsg),
            }
        }
    });
    let _ = drain.await;

    client.log_off()?;
    Ok(())
}
