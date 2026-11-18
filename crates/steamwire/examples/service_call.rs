//! Unified messages before and after logon, by method name and through the
//! typed service clients, and a heartbeat check.
//!
//! ```sh
//! cargo run -p steamwire --example service_call
//! ```

use std::time::Duration;

use steamwire::protos::steam::{
    CAuthenticationGetPasswordRsaPublicKeyRequest, CAuthenticationGetPasswordRsaPublicKeyResponse,
    CContentServerDirectoryGetServersForSteamPipeRequest,
};
use steamwire::services::ContentServerDirectory;
use steamwire::{Config, SteamClient};

#[tokio::main]
async fn main() -> steamwire::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let (client, _events) = SteamClient::connect(Config::default()).await?;

    // Not logged on yet: sent as ServiceMethodCallFromClientNonAuthed.
    // By method name, with the response type spelled out:
    let key: CAuthenticationGetPasswordRsaPublicKeyResponse = client
        .call_service(
            "Authentication.GetPasswordRSAPublicKey#1",
            CAuthenticationGetPasswordRsaPublicKeyRequest { account_name: Some("steam".into()) },
        )
        .await?;
    println!(
        "RSA key: {} hex digits, exponent {}, timestamp {}",
        key.publickey_mod().len(),
        key.publickey_exp(),
        key.timestamp()
    );

    client.log_on_anonymous().await?;
    println!("logged on as {}", client.steam_id().unwrap());

    // Through the typed client of the service:
    let servers = ContentServerDirectory::new(&client)
        .get_servers_for_steam_pipe(CContentServerDirectoryGetServersForSteamPipeRequest {
            cell_id: client.cell_id(),
            max_servers: Some(5),
            ..Default::default()
        })
        .await?;
    for server in &servers.servers {
        println!("CDN server: {} ({})", server.host(), server.r#type());
    }

    let soak = std::env::var("SOAK_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(35);
    println!("idling {soak}s to check heartbeats...");
    tokio::time::sleep(Duration::from_secs(soak)).await;
    let info = client.pics_get_product_info(&[730.into()], &[]).await?;
    println!(
        "still connected: {}, app 730 is {:?}",
        client.is_connected(),
        info.apps[&730].key_values["common"]["name"].as_str()
    );

    client.log_off()?;
    Ok(())
}
