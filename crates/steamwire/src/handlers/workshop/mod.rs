//! Workshop (SteamKit2's `SteamWorkshop`).

use crate::protos::steam::{
    CMsgClientUcmEnumeratePublishedFilesByUserAction, CMsgClientUcmEnumeratePublishedFilesByUserActionResponse,
};

use crate::client::SteamClient;
use crate::enums::{EMsg, EResult, EWorkshopFileAction};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamWorkshop`.
    pub enum WorkshopCallback {
        /// `ClientUCMEnumeratePublishedFilesByUserActionResponse`.
        UserActionPublishedFiles(CMsgClientUcmEnumeratePublishedFilesByUserActionResponse) =
            ClientUCMEnumeratePublishedFilesByUserActionResponse,
    }
}

impl WorkshopCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

impl SteamClient {
    /// Lists files the user played, completed, ... in an app.
    pub async fn enumerate_published_files_by_user_action(
        &self,
        app_id: u32,
        action: EWorkshopFileAction,
        start_index: u32,
    ) -> Result<CMsgClientUcmEnumeratePublishedFilesByUserActionResponse> {
        let response: CMsgClientUcmEnumeratePublishedFilesByUserActionResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientUCMEnumeratePublishedFilesByUserAction,
                CMsgClientUcmEnumeratePublishedFilesByUserAction {
                    app_id: Some(app_id),
                    start_index: Some(start_index),
                    action: Some(action.0),
                },
            ))
            .await?
            .decode()?;
        match EResult(response.eresult()) {
            EResult::OK => Ok(response),
            other => Err(Error::Steam(other)),
        }
    }
}
