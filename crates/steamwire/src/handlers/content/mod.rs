//! Content server directory calls (SteamKit2's `SteamContent`): CDN servers,
//! manifest request codes and CDN auth tokens.

use steamwire_protos::steam::{
    CContentServerDirectoryGetCdnAuthTokenRequest, CContentServerDirectoryGetCdnAuthTokenResponse,
    CContentServerDirectoryGetManifestRequestCodeRequest, CContentServerDirectoryGetManifestRequestCodeResponse,
    CContentServerDirectoryGetServersForSteamPipeRequest, CContentServerDirectoryGetServersForSteamPipeResponse,
};

use crate::cdn::Server;
use crate::client::SteamClient;
use crate::error::{Error, Result};

/// A token for CDN servers that need one.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CdnAuthToken {
    /// The token, appended to chunk URLs.
    pub token: String,
    /// Unix time it expires.
    pub expiration: u32,
}

impl SteamClient {
    /// Lists content servers. `cell_id` defaults to the session's cell.
    pub async fn get_servers_for_steam_pipe(
        &self,
        cell_id: Option<u32>,
        max_servers: Option<u32>,
    ) -> Result<Vec<Server>> {
        let response: CContentServerDirectoryGetServersForSteamPipeResponse = self
            .call_service(
                "ContentServerDirectory.GetServersForSteamPipe#1",
                CContentServerDirectoryGetServersForSteamPipeRequest {
                    cell_id: Some(cell_id.unwrap_or_else(|| self.cell_id().unwrap_or(0))),
                    max_servers,
                    ..Default::default()
                },
            )
            .await?;
        Ok(response.servers.iter().map(Server::from_proto).collect())
    }

    /// Requests the code needed to download a manifest. `branch` of
    /// `"public"` is the same as none.
    pub async fn get_manifest_request_code(
        &self,
        depot_id: u32,
        app_id: u32,
        manifest_id: u64,
        branch: Option<&str>,
        branch_password_hash: Option<&str>,
    ) -> Result<u64> {
        let (branch, hash) = match branch {
            Some(b) if b.eq_ignore_ascii_case("public") => (None, None),
            b => (b, branch_password_hash),
        };
        if hash.is_some() && branch.is_none() {
            return Err(Error::Other("a branch password needs a branch".into()));
        }
        let response: CContentServerDirectoryGetManifestRequestCodeResponse = self
            .call_service(
                "ContentServerDirectory.GetManifestRequestCode#1",
                CContentServerDirectoryGetManifestRequestCodeRequest {
                    app_id: Some(app_id),
                    depot_id: Some(depot_id),
                    manifest_id: Some(manifest_id),
                    app_branch: branch.map(str::to_string),
                    branch_password_hash: hash.map(str::to_string),
                },
            )
            .await?;
        Ok(response.manifest_request_code())
    }

    /// Requests a CDN auth token for a depot on a host.
    pub async fn get_cdn_auth_token(&self, app_id: u32, depot_id: u32, host_name: &str) -> Result<CdnAuthToken> {
        let response: CContentServerDirectoryGetCdnAuthTokenResponse = self
            .call_service(
                "ContentServerDirectory.GetCDNAuthToken#1",
                CContentServerDirectoryGetCdnAuthTokenRequest {
                    app_id: Some(app_id),
                    depot_id: Some(depot_id),
                    host_name: Some(host_name.to_string()),
                },
            )
            .await?;
        Ok(CdnAuthToken { token: response.token().to_string(), expiration: response.expiration_time() })
    }
}
