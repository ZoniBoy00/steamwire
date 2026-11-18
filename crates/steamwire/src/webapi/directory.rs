//! `ISteamDirectory`: CM server lists.

use serde::Deserialize;

use super::WebApi;
use crate::error::{Error, Result};
use crate::net::{CmServer, ProtocolType};

impl WebApi {
    /// Fetches CM servers from `ISteamDirectory/GetCMListForConnect`, best
    /// first (SteamKit2's `SteamDirectory.LoadAsync`). `protocol` filters by
    /// transport, `None` returns both; `max_servers` limits the count.
    pub async fn cm_servers(
        &self,
        cell_id: u32,
        protocol: Option<ProtocolType>,
        max_servers: Option<u32>,
    ) -> Result<Vec<CmServer>> {
        #[derive(Deserialize)]
        struct Envelope {
            response: CmListResponse,
        }
        #[derive(Deserialize)]
        struct CmListResponse {
            #[serde(default)]
            serverlist: Vec<CmListEntry>,
            #[serde(default)]
            success: bool,
            #[serde(default)]
            message: String,
        }
        #[derive(Deserialize)]
        struct CmListEntry {
            endpoint: String,
            #[serde(rename = "type")]
            kind: String,
        }

        let cell = cell_id.to_string();
        let max = max_servers.map(|m| m.to_string());
        let mut args = vec![("cellid", cell.as_str())];
        if let Some(max) = &max {
            args.push(("maxcount", max.as_str()));
        }
        let envelope: Envelope = self.get("ISteamDirectory", "GetCMListForConnect", 1, &args).await?;
        if !envelope.response.success {
            return Err(Error::Other(format!("GetCMListForConnect failed: {}", envelope.response.message)));
        }

        let servers = envelope
            .response
            .serverlist
            .into_iter()
            .filter_map(|e| {
                let p = match e.kind.as_str() {
                    "websockets" => ProtocolType::WebSocket,
                    "netfilter" => ProtocolType::Tcp,
                    _ => return None,
                };
                (protocol.is_none() || protocol == Some(p)).then_some(CmServer { endpoint: e.endpoint, protocol: p })
            })
            .collect();
        Ok(servers)
    }
}
