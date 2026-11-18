//! Networking certificates (SteamKit2's `SteamNetworking`).

use steamwire_protos::steam::{CMsgClientNetworkingCertReply, CMsgClientNetworkingCertRequest};

use crate::client::SteamClient;
use crate::enums::EMsg;
use crate::error::Result;
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamNetworking`.
    pub enum NetworkingCallback {
        /// `ClientNetworkingCertRequestResponse`.
        NetworkingCertificate(CMsgClientNetworkingCertReply) = ClientNetworkingCertRequestResponse,
    }
}

impl NetworkingCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

impl SteamClient {
    /// Requests a signed networking certificate for a public key
    /// (Steam Datagram Relay).
    pub async fn request_networking_certificate(
        &self,
        app_id: u32,
        public_key: &[u8],
    ) -> Result<CMsgClientNetworkingCertReply> {
        self.call(ProtoMsg::new(
            EMsg::ClientNetworkingCertRequest,
            CMsgClientNetworkingCertRequest { key_data: Some(public_key.to_vec()), app_id: Some(app_id) },
        ))
        .await?
        .decode()
    }
}
