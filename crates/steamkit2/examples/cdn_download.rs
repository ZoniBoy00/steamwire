//! Downloads a depot manifest and a file from the CDN with an anonymous
//! account: Spacewar (app 480), depot 481.
//!
//! ```sh
//! cargo run -p steamkit2 --example cdn_download
//! ```

use sha1::{Digest, Sha1};
use steamkit2::cdn::CdnClient;
use steamkit2::{Config, SteamClient};

const APP_ID: u32 = 480;
const DEPOT_ID: u32 = 481;

#[tokio::main]
async fn main() -> steamkit2::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let (client, _events) = SteamClient::connect(Config::default()).await?;
    client.log_on_anonymous().await?;

    // The public manifest ID: newer product info has `gid` below the branch.
    let info = client.pics_get_product_info(&[APP_ID.into()], &[]).await?;
    let public = &info.apps[&APP_ID].key_values["depots"][&*DEPOT_ID.to_string()]["manifests"]["public"];
    let manifest_id: u64 = public["gid"]
        .as_str()
        .or_else(|| public.as_str())
        .and_then(|s| s.parse().ok())
        .expect("public manifest of depot 481");
    println!("depot {DEPOT_ID} public manifest {manifest_id}");

    let depot_key = client.get_depot_decryption_key(DEPOT_ID, APP_ID).await?;
    let request_code = client.get_manifest_request_code(DEPOT_ID, APP_ID, manifest_id, None, None).await?;
    let servers = client.get_servers_for_steam_pipe(None, None).await?;
    println!("depot key: {} bytes, request code: {}, {} servers", depot_key.len(), request_code != 0, servers.len());

    let cdn = CdnClient::new();
    let mut last_error = None;
    for server in servers.iter().filter(|s| s.allowed_app_ids.is_empty() || s.allowed_app_ids.contains(&APP_ID)).take(5)
    {
        let manifest = match cdn
            .download_manifest(DEPOT_ID, manifest_id, request_code, server, Some(&depot_key), None, None)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                println!("{server}: {e}");
                last_error = Some(e);
                continue;
            }
        };
        println!(
            "manifest from {server}: {} files, {} bytes, created {}",
            manifest.files.len(),
            manifest.total_uncompressed_size,
            manifest.creation_time
        );
        for file in manifest.files.iter().take(10) {
            println!("  {} ({} bytes, {} chunks)", file.file_name, file.total_size, file.chunks.len());
        }

        // Download the smallest and the largest file and check their SHA-1.
        let with_chunks = || manifest.files.iter().filter(|f| !f.chunks.is_empty());
        let smallest = with_chunks().min_by_key(|f| f.total_size).expect("a file with chunks");
        let largest = with_chunks().max_by_key(|f| f.total_size).expect("a file with chunks");
        for file in [smallest, largest] {
            let mut contents = vec![0u8; file.total_size as usize];
            for chunk in &file.chunks {
                let data = cdn.download_depot_chunk(DEPOT_ID, chunk, server, Some(&depot_key), None, None).await?;
                let at = chunk.offset as usize;
                contents[at..at + data.len()].copy_from_slice(&data);
            }
            let ok = Sha1::digest(&contents).as_slice() == file.file_hash.as_slice();
            println!("downloaded {} ({} bytes), SHA-1 matches: {ok}", file.file_name, contents.len());
        }

        client.log_off()?;
        return Ok(());
    }
    Err(last_error.unwrap_or(steamkit2::Error::Other("no usable content server".into())))
}
