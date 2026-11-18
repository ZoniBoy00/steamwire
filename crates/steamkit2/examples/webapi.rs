//! Web API calls without a key: JSON, KeyValues and protobuf.
//!
//! ```sh
//! cargo run -p steamkit2 --example webapi
//! ```

use steamkit2::handlers::auth::guard;
use steamkit2::protos::steam::{
    CContentServerDirectoryGetServersForSteamPipeRequest, CContentServerDirectoryGetServersForSteamPipeResponse,
};
use steamkit2::webapi::{HttpMethod, WebApi};

#[tokio::main]
async fn main() -> steamkit2::Result<()> {
    let api = WebApi::new();

    let json: serde_json::Value = api.get("ISteamWebAPIUtil", "GetServerInfo", 1, &[]).await?;
    println!("JSON: server time {}", json["servertime"]);

    let kv = api.interface("ISteamWebAPIUtil").call(HttpMethod::Get, "GetServerInfo", 1, &[]).await?;
    println!("KeyValues: server time {:?}, {:?}", kv["servertime"].as_u64(), kv["servertimestring"].as_str());

    let request = CContentServerDirectoryGetServersForSteamPipeRequest { cell_id: Some(0), ..Default::default() };
    let response = api
        .interface("IContentServerDirectoryService")
        .call_protobuf::<_, CContentServerDirectoryGetServersForSteamPipeResponse>(
            HttpMethod::Get,
            "GetServersForSteamPipe",
            1,
            &request,
        )
        .await?;
    println!("protobuf: {:?}, {} content servers", response.result, response.body.servers.len());
    if let Some(server) = response.body.servers.first() {
        println!("  first: {} ({})", server.host(), server.r#type());
    }

    let servers = api.content_servers(0, Some(3)).await?;
    println!("content_servers: {:?}", servers.iter().map(|s| &s.host).collect::<Vec<_>>());

    println!("Steam Guard clock offset: {} s", guard::server_time_offset(&api).await?);
    Ok(())
}
