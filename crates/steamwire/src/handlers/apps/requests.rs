use crate::protos::steam::{
    CMsgClientCheckAppBetaPassword, CMsgClientCheckAppBetaPasswordResponse, CMsgClientGetAppOwnershipTicket,
    CMsgClientGetAppOwnershipTicketResponse, CMsgClientGetDepotDecryptionKey, CMsgClientGetDepotDecryptionKeyResponse,
    CMsgClientPicsChangesSinceRequest, CMsgClientPicsChangesSinceResponse, CMsgClientPicsPrivateBetaRequest,
    CMsgClientRequestFreeLicense, CMsgClientRequestFreeLicenseResponse,
};

use super::callbacks::PrivateBeta;
use crate::client::SteamClient;
use crate::enums::{EMsg, EResult};
use crate::error::{Error, Result};
use crate::messages::generated::{MsgClientGetLegacyGameKey, MsgClientGetLegacyGameKeyResponse};
use crate::messages::{ClientMsg, ProtoMsg};

pub(crate) fn check(result: i32) -> Result<()> {
    match EResult(result) {
        EResult::OK => Ok(()),
        other => Err(Error::Steam(other)),
    }
}

impl SteamClient {
    /// Requests an app ownership ticket.
    pub async fn get_app_ownership_ticket(&self, app_id: u32) -> Result<CMsgClientGetAppOwnershipTicketResponse> {
        let response: CMsgClientGetAppOwnershipTicketResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientGetAppOwnershipTicket,
                CMsgClientGetAppOwnershipTicket { app_id: Some(app_id) },
            ))
            .await?
            .decode()?;
        check(response.eresult() as i32)?;
        Ok(response)
    }

    /// Requests the decryption key of a depot.
    pub async fn get_depot_decryption_key(&self, depot_id: u32, app_id: u32) -> Result<Vec<u8>> {
        let response: CMsgClientGetDepotDecryptionKeyResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientGetDepotDecryptionKey,
                CMsgClientGetDepotDecryptionKey { depot_id: Some(depot_id), app_id: Some(app_id) },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        Ok(response.depot_encryption_key.unwrap_or_default())
    }

    /// Requests the app and package changes since a change number.
    pub async fn pics_get_changes_since(
        &self,
        since_change_number: u32,
        send_app_changes: bool,
        send_package_changes: bool,
    ) -> Result<CMsgClientPicsChangesSinceResponse> {
        self.call(ProtoMsg::new(
            EMsg::ClientPICSChangesSinceRequest,
            CMsgClientPicsChangesSinceRequest {
                since_change_number: Some(since_change_number),
                send_app_info_changes: Some(send_app_changes),
                send_package_info_changes: Some(send_package_changes),
                ..Default::default()
            },
        ))
        .await?
        .decode()
    }

    /// Requests the depot section of a password protected beta branch.
    pub async fn pics_get_private_beta(
        &self,
        app_id: u32,
        access_token: u64,
        branch: &str,
        branch_password_hash: &[u8],
    ) -> Result<PrivateBeta> {
        let packet = self
            .call(ProtoMsg::new(
                EMsg::ClientPICSPrivateBetaRequest,
                CMsgClientPicsPrivateBetaRequest {
                    appid: Some(app_id),
                    access_token: Some(access_token),
                    beta_name: Some(branch.to_string()),
                    password_hash: Some(branch_password_hash.to_vec()),
                },
            ))
            .await?;
        let beta = PrivateBeta::parse(&packet).ok_or(Error::Malformed("bad private beta response"))?;
        check(beta.result.0)?;
        Ok(beta)
    }

    /// Requests free licenses for apps. The response lists the granted
    /// package and app IDs.
    pub async fn request_free_license(&self, app_ids: &[u32]) -> Result<CMsgClientRequestFreeLicenseResponse> {
        let response: CMsgClientRequestFreeLicenseResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientRequestFreeLicense,
                CMsgClientRequestFreeLicense { appids: app_ids.to_vec() },
            ))
            .await?
            .decode()?;
        check(response.eresult() as i32)?;
        Ok(response)
    }

    /// Checks a beta branch password. The response holds the unlocked
    /// branch names and their keys.
    pub async fn check_app_beta_password(
        &self,
        app_id: u32,
        password: &str,
    ) -> Result<CMsgClientCheckAppBetaPasswordResponse> {
        let response: CMsgClientCheckAppBetaPasswordResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientCheckAppBetaPassword,
                CMsgClientCheckAppBetaPassword {
                    app_id: Some(app_id),
                    betapassword: Some(password.to_string()),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        Ok(response)
    }

    /// Requests the legacy CD key of an app.
    pub async fn get_legacy_game_key(&self, app_id: u32) -> Result<String> {
        let packet = self.call(ClientMsg::new(MsgClientGetLegacyGameKey { app_id })).await?;
        let msg = ClientMsg::<MsgClientGetLegacyGameKeyResponse>::from_packet(&packet)?;
        if msg.body.result != EResult::OK {
            return Err(Error::Steam(msg.body.result));
        }
        let len = (msg.body.length as usize).saturating_sub(1).min(msg.payload.len());
        Ok(String::from_utf8_lossy(&msg.payload[..len]).into_owned())
    }
}
