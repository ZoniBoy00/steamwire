//! Game server logon and status (SteamKit2's `SteamGameServer`).

use std::net::Ipv4Addr;

use steamkit_protos::steam::{
    CMsgClientLogOff, CMsgClientLogon, CMsgClientTicketAuthComplete, CMsgGsServerType, CMsgGsStatusReply,
};

use super::user::{default_os_type, LoggedOn};
use crate::client::{SteamClient, CURRENT_PROTOCOL};
use crate::enums::{EAccountType, EMsg, EServerFlags};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};
use crate::types::SteamId;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamGameServer`.
    pub enum GameServerCallback {
        /// `GSStatusReply`.
        StatusReply(CMsgGsStatusReply) = GSStatusReply,
        /// `ClientTicketAuthComplete`: a user's auth ticket was checked.
        TicketAuth(CMsgClientTicketAuthComplete) = ClientTicketAuthComplete,
    }
}

impl GameServerCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

/// Game server status, sent with [`SteamClient::send_game_server_status`].
#[derive(Clone, Debug, Default)]
pub struct StatusDetails {
    /// The app served.
    pub app_id: u32,
    /// Server flags.
    pub server_flags: EServerFlags,
    /// The game directory, such as `tf`.
    pub game_directory: String,
    /// The address, if not the one Steam sees.
    pub address: Option<Ipv4Addr>,
    /// The game port.
    pub port: u32,
    /// The query port.
    pub query_port: u32,
    /// The game version.
    pub version: String,
}

impl SteamClient {
    fn game_server_logon(&self, app_id: u32) -> CMsgClientLogon {
        CMsgClientLogon {
            obfuscated_private_ip: Some(self.obfuscated_local_ip()),
            protocol_version: Some(CURRENT_PROTOCOL),
            client_os_type: Some(default_os_type().0 as u32),
            game_server_app_id: Some(app_id as i32),
            machine_id: Some(self.machine_id()),
            ..Default::default()
        }
    }

    /// Logs on as a persistent game server with a game server login token.
    pub async fn log_on_game_server(&self, app_id: u32, token: &str) -> Result<LoggedOn> {
        if token.is_empty() {
            return Err(Error::Other("game server logon needs a token".into()));
        }
        let steam_id = SteamId::new(0, 0, self.universe(), EAccountType::GameServer);
        let mut body = self.game_server_logon(app_id);
        body.game_server_token = Some(token.to_string());
        self.send_logon_as(EMsg::ClientLogonGameServer, steam_id, body).await
    }

    /// Logs on as an anonymous game server.
    pub async fn log_on_game_server_anonymous(&self, app_id: u32) -> Result<LoggedOn> {
        let steam_id = SteamId::new(0, 0, self.universe(), EAccountType::AnonGameServer);
        self.send_logon_as(EMsg::ClientLogon, steam_id, self.game_server_logon(app_id)).await
    }

    /// Logs off a game server.
    pub fn log_off_game_server(&self) -> Result<()> {
        self.expect_disconnect();
        self.send(ProtoMsg::new(EMsg::ClientLogOff, CMsgClientLogOff::default()))
    }

    /// Sends the game server's status.
    pub fn send_game_server_status(&self, details: &StatusDetails) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::GSServerType,
            CMsgGsServerType {
                app_id_served: Some(details.app_id),
                flags: Some(details.server_flags.0 as u32),
                game_dir: Some(details.game_directory.clone()),
                game_port: Some(details.port),
                game_query_port: Some(details.query_port),
                game_version: Some(details.version.clone()),
                deprecated_game_ip_address: details.address.map(u32::from),
                ..Default::default()
            },
        ))
    }
}
