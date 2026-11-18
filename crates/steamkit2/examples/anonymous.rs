//! Connects to Steam, logs on anonymously, fetches Spacewar's app info over
//! PICS, then logs off.
//!
//! ```sh
//! cargo run -p steamkit2 --example anonymous          # WebSocket
//! cargo run -p steamkit2 --example anonymous -- tcp   # encrypted TCP
//! cargo run -p steamkit2 --example anonymous -- udp   # encrypted UDP
//! cargo run -p steamkit2 --example anonymous -- tcp host:port   # a given server
//! ```
//!
//! With `NETHOOK=1`, every message is dumped to `nethook/<time>/` next to
//! the executable, for `steamkit2-tools nethook`.

use std::sync::Arc;

use steamkit2::{CmServer, Config, Event, NetHookListener, ProtocolType, SteamClient};

#[tokio::main]
async fn main() -> steamkit2::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,steamkit2=debug".into()),
        )
        .init();

    let protocol = match std::env::args().nth(1).as_deref() {
        Some("tcp") => ProtocolType::Tcp,
        Some("udp") => ProtocolType::Udp,
        _ => ProtocolType::WebSocket,
    };
    let server = std::env::args().nth(2).map(|endpoint| CmServer { endpoint, protocol });

    let network_listener = match std::env::var_os("NETHOOK") {
        Some(_) => {
            let listener = NetHookListener::new()?;
            println!("dumping messages to {}", listener.directory().display());
            Some(Arc::new(listener) as _)
        }
        None => None,
    };

    let config = Config { protocols: protocol.into(), server, network_listener, ..Config::default() };
    let (client, mut events) = SteamClient::connect(config).await?;
    println!("connected to {} over {:?}", client.server().endpoint, client.server().protocol);

    let logged_on = client.log_on_anonymous().await?;
    println!(
        "logged on as {} (cell {}, heartbeat every {}s)",
        logged_on.steam_id,
        logged_on.response.cell_id(),
        logged_on.response.legacy_out_of_game_heartbeat_seconds()
    );

    let info = client.pics_get_product_info(&[480.into()], &[]).await?;
    let app = &info.apps[&480];
    println!(
        "app 480: {:?}, change number {}, {} depots",
        app.key_values["common"]["name"].as_str(),
        app.change_number,
        app.key_values["depots"].children().len()
    );

    client.log_off()?;
    while let Some(event) = events.recv().await {
        match event {
            Event::Message(packet) => println!("received {:?}", packet.emsg),
            Event::Disconnected { user_initiated } => {
                println!("disconnected (user initiated: {user_initiated})");
                break;
            }
        }
    }
    Ok(())
}
