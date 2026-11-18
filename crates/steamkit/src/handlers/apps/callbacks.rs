use steamkit_protos::steam::{
    CMsgClientCheckAppBetaPasswordResponse, CMsgClientGameConnectTokens, CMsgClientGetAppOwnershipTicketResponse,
    CMsgClientGetDepotDecryptionKeyResponse, CMsgClientLicenseList, CMsgClientPicsAccessTokenResponse,
    CMsgClientPicsChangesSinceResponse, CMsgClientPicsPrivateBetaResponse, CMsgClientPurchaseResponse,
    CMsgClientRedeemGuestPassResponse, CMsgClientRequestFreeLicenseResponse,
};

use super::{parse_text_buffer, PicsProductInfoResult};
use crate::enums::{EMsg, EResult};
use crate::handlers::proto_callbacks;
use crate::messages::generated::{MsgClientUpdateGuestPassesList, MsgClientVACBanStatus};
use crate::messages::{ClientMsg, Packet, Reader};
use crate::types::kv::KeyValue;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamApps`.
    pub enum AppsCallback {
        /// `ClientLicenseList`: the account's licenses, sent after logon.
        LicenseList(CMsgClientLicenseList) = ClientLicenseList,
        /// `ClientGameConnectTokens`.
        GameConnectTokens(CMsgClientGameConnectTokens) = ClientGameConnectTokens,
        /// `ClientGetAppOwnershipTicketResponse`.
        AppOwnershipTicket(CMsgClientGetAppOwnershipTicketResponse) = ClientGetAppOwnershipTicketResponse,
        /// `ClientGetDepotDecryptionKeyResponse`.
        DepotKey(CMsgClientGetDepotDecryptionKeyResponse) = ClientGetDepotDecryptionKeyResponse,
        /// `ClientPICSAccessTokenResponse`.
        PicsTokens(CMsgClientPicsAccessTokenResponse) = ClientPICSAccessTokenResponse,
        /// `ClientPICSChangesSinceResponse`.
        PicsChanges(CMsgClientPicsChangesSinceResponse) = ClientPICSChangesSinceResponse,
        /// `ClientPurchaseResponse`.
        Purchase(CMsgClientPurchaseResponse) = ClientPurchaseResponse,
        /// `ClientRedeemGuestPassResponse`.
        RedeemGuestPass(CMsgClientRedeemGuestPassResponse) = ClientRedeemGuestPassResponse,
        /// `ClientRequestFreeLicenseResponse`.
        FreeLicense(CMsgClientRequestFreeLicenseResponse) = ClientRequestFreeLicenseResponse,
        /// `ClientCheckAppBetaPasswordResponse`.
        CheckAppBetaPassword(CMsgClientCheckAppBetaPasswordResponse) = ClientCheckAppBetaPasswordResponse,
    }
    extra {
        /// `ClientPICSProductInfoResponse` (one part of a response).
        PicsProductInfo(PicsProductInfoResult),
        /// `ClientPICSPrivateBetaResponse`.
        PrivateBeta(PrivateBeta),
        /// `ClientVACBanStatus`: apps the account is VAC banned in.
        VacStatus(Vec<u32>),
        /// `ClientUpdateGuestPassesList`.
        GuestPassList(GuestPassList),
        /// `ClientGetLegacyGameKeyResponse`.
        LegacyGameKey(LegacyGameKey),
    }
}

impl AppsCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        match packet.emsg {
            EMsg::ClientPICSProductInfoResponse => {
                let mut result = PicsProductInfoResult::default();
                result.extend(packet.decode().ok()?);
                Some(Self::PicsProductInfo(result))
            }
            EMsg::ClientPICSPrivateBetaResponse => PrivateBeta::parse(packet).map(Self::PrivateBeta),
            EMsg::ClientVACBanStatus => {
                let msg = ClientMsg::<MsgClientVACBanStatus>::from_packet(packet).ok()?;
                let mut r = Reader::new(&msg.payload);
                let bans = (0..msg.body.num_bans).map(|_| r.u32()).collect::<Result<_, _>>().ok()?;
                Some(Self::VacStatus(bans))
            }
            EMsg::ClientUpdateGuestPassesList => GuestPassList::parse(packet).map(Self::GuestPassList),
            EMsg::ClientGetLegacyGameKeyResponse => LegacyGameKey::parse(packet).map(Self::LegacyGameKey),
            _ => Self::decode_proto(packet),
        }
    }
}

/// The depot section of a private beta branch.
#[derive(Clone, Debug)]
pub struct PrivateBeta {
    /// The result.
    pub result: EResult,
    /// The depots of the branch.
    pub depot_section: KeyValue,
}

impl PrivateBeta {
    pub(super) fn parse(packet: &Packet) -> Option<Self> {
        let msg: CMsgClientPicsPrivateBetaResponse = packet.decode().ok()?;
        let depot_section = msg
            .depot_section
            .as_deref()
            .and_then(parse_text_buffer)
            .unwrap_or_else(|| KeyValue::new_object("", Vec::new()));
        Some(Self { result: EResult(msg.eresult()), depot_section })
    }
}

/// The guest passes of the account.
#[derive(Clone, Debug)]
pub struct GuestPassList {
    /// The result.
    pub result: EResult,
    /// Passes the account can give.
    pub count_to_give: i32,
    /// Passes the account can redeem.
    pub count_to_redeem: i32,
    /// One KeyValues node per pass.
    pub guest_passes: Vec<KeyValue>,
}

impl GuestPassList {
    fn parse(packet: &Packet) -> Option<Self> {
        let msg = ClientMsg::<MsgClientUpdateGuestPassesList>::from_packet(packet).ok()?;
        let body = &msg.body;
        let mut data = msg.payload.as_slice();
        let mut guest_passes = Vec::new();
        for _ in 0..(body.count_guest_passes_to_give + body.count_guest_passes_to_redeem).max(0) {
            let (kv, used) = KeyValue::parse_binary_prefix(data).ok()?;
            guest_passes.push(kv);
            data = &data[used..];
        }
        Some(Self {
            result: body.result,
            count_to_give: body.count_guest_passes_to_give,
            count_to_redeem: body.count_guest_passes_to_redeem,
            guest_passes,
        })
    }
}

/// A legacy CD key.
#[derive(Clone, Debug)]
pub struct LegacyGameKey {
    /// The result.
    pub result: EResult,
    /// The app.
    pub app_id: u32,
    /// The key, if any.
    pub key: Option<String>,
}

impl LegacyGameKey {
    fn parse(packet: &Packet) -> Option<Self> {
        use crate::messages::generated::MsgClientGetLegacyGameKeyResponse;
        let msg = ClientMsg::<MsgClientGetLegacyGameKeyResponse>::from_packet(packet).ok()?;
        let key = (msg.body.length > 0).then(|| {
            let len = (msg.body.length as usize - 1).min(msg.payload.len());
            String::from_utf8_lossy(&msg.payload[..len]).into_owned()
        });
        Some(Self { result: msg.body.result, app_id: msg.body.app_id, key })
    }
}
