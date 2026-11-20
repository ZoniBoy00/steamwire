//! Game server queries (SteamKit2's `SteamMasterServer`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::protos::steam::{CMsgClientGmsServerQuery, CMsgGmsClientServerQueryResponse};

use crate::client::SteamClient;
use crate::enums::{EMsg, ERegionCode};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};
use crate::util::net::ip_from_msg;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamMasterServer`.
    pub enum MasterServerCallback {
        /// `GMSClientServerQueryResponse`.
        Query(CMsgGmsClientServerQueryResponse) = GMSClientServerQueryResponse,
    }
}

impl MasterServerCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

/// A server query.
#[derive(Clone, Debug)]
pub struct QueryDetails {
    /// The app.
    pub app_id: u32,
    /// The filter, in master server syntax (`\gamedir\tf\empty\1`).
    pub filter: String,
    /// The region.
    pub region: ERegionCode,
    /// Sort by distance to this IP.
    pub geo_located_ip: Option<Ipv4Addr>,
    /// The result limit.
    pub max_servers: u32,
}

/// A server from a query.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Server {
    /// The query address.
    pub endpoint: SocketAddr,
    /// Authenticated players.
    pub authed_players: u32,
}

impl SteamClient {
    /// Queries the master server for game servers.
    pub async fn server_query(&self, details: &QueryDetails) -> Result<Vec<Server>> {
        let response: CMsgGmsClientServerQueryResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientGMSServerQuery,
                CMsgClientGmsServerQuery {
                    app_id: Some(details.app_id),
                    geo_location_ip: details.geo_located_ip.map(u32::from),
                    filter_text: Some(details.filter.clone()),
                    region_code: Some(details.region.0 as u32),
                    max_servers: Some(details.max_servers),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        if let Some(error) = response.error.filter(|e| !e.is_empty()) {
            return Err(Error::Other(error));
        }
        Ok(response
            .servers
            .iter()
            .map(|s| {
                let ip = s
                    .server_ip
                    .as_ref()
                    .and_then(ip_from_msg)
                    .unwrap_or_else(|| IpAddr::V4(Ipv4Addr::from(s.deprecated_server_ip())));
                Server { endpoint: SocketAddr::new(ip, s.query_port() as u16), authed_players: s.auth_players() }
            })
            .collect())
    }
}
