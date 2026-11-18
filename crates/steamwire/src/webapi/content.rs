//! `IContentServerDirectoryService`: CDN server lists.

use steamwire_protos::steam::{
    CContentServerDirectoryGetServersForSteamPipeRequest, CContentServerDirectoryGetServersForSteamPipeResponse,
};

use super::{HttpMethod, WebApi};
use crate::cdn::Server;
use crate::error::Result;

impl WebApi {
    /// Fetches CDN servers from
    /// `IContentServerDirectoryService/GetServersForSteamPipe` without a CM
    /// connection (SteamKit2's `ContentServerDirectoryService.LoadAsync`).
    /// `max_servers` is passed on as a hint: like SteamKit2, the result is not
    /// cut, and Steam may return more.
    pub async fn content_servers(&self, cell_id: u32, max_servers: Option<u32>) -> Result<Vec<Server>> {
        let request = CContentServerDirectoryGetServersForSteamPipeRequest {
            cell_id: Some(cell_id),
            max_servers,
            ..Default::default()
        };
        let response = self
            .interface("IContentServerDirectoryService")
            .call_protobuf::<_, CContentServerDirectoryGetServersForSteamPipeResponse>(
                HttpMethod::Get,
                "GetServersForSteamPipe",
                1,
                &request,
            )
            .await?;
        Ok(response.body.servers.iter().map(Server::from_proto).collect())
    }
}
