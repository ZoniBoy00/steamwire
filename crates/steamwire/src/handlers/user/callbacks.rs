use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::protos::steam::{
    CMsgClientAccountInfo, CMsgClientEmailAddrInfo, CMsgClientLoggedOff, CMsgClientLogonResponse,
    CMsgClientPlayingSessionState, CMsgClientRequestWebApiAuthenticateUserNonceResponse, CMsgClientSessionToken,
    CMsgClientVanityUrlChangedNotification, CMsgClientWalletInfoUpdate,
};

use crate::enums::{EMarketingMessageFlags, EMsg};
use crate::handlers::proto_callbacks;
use crate::messages::generated::MsgClientMarketingMessageUpdate2;
use crate::messages::{ClientMsg, Packet, Reader};
use crate::types::GlobalId;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamUser`.
    pub enum UserCallback {
        /// `ClientLogOnResponse` (also returned by `log_on`).
        LoggedOn(CMsgClientLogonResponse) = ClientLogOnResponse,
        /// `ClientLoggedOff`: the server ended the session.
        LoggedOff(CMsgClientLoggedOff) = ClientLoggedOff,
        /// `ClientSessionToken`.
        SessionToken(CMsgClientSessionToken) = ClientSessionToken,
        /// `ClientAccountInfo`: persona name, country, flags.
        AccountInfo(CMsgClientAccountInfo) = ClientAccountInfo,
        /// `ClientEmailAddrInfo`.
        EmailAddrInfo(CMsgClientEmailAddrInfo) = ClientEmailAddrInfo,
        /// `ClientWalletInfoUpdate`.
        WalletInfo(CMsgClientWalletInfoUpdate) = ClientWalletInfoUpdate,
        /// `ClientRequestWebAPIAuthenticateUserNonceResponse`.
        WebApiUserNonce(CMsgClientRequestWebApiAuthenticateUserNonceResponse) = ClientRequestWebAPIAuthenticateUserNonceResponse,
        /// `ClientVanityURLChangedNotification`.
        VanityUrlChanged(CMsgClientVanityUrlChangedNotification) = ClientVanityURLChangedNotification,
        /// `ClientPlayingSessionState`: whether a game is being played elsewhere.
        PlayingSessionState(CMsgClientPlayingSessionState) = ClientPlayingSessionState,
    }
    extra {
        /// `ClientMarketingMessageUpdate2`.
        MarketingMessages(MarketingMessages),
    }
}

impl UserCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        if packet.emsg == EMsg::ClientMarketingMessageUpdate2 {
            return MarketingMessages::parse(packet).map(Self::MarketingMessages);
        }
        Self::decode_proto(packet)
    }
}

/// One marketing message.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MarketingMessage {
    /// The message ID.
    pub id: GlobalId,
    /// The URL to show.
    pub url: String,
    /// Flags.
    pub flags: EMarketingMessageFlags,
}

/// The marketing messages list.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MarketingMessages {
    /// When the list was updated.
    pub update_time: SystemTime,
    /// The messages.
    pub messages: Vec<MarketingMessage>,
}

impl MarketingMessages {
    fn parse(packet: &Packet) -> Option<Self> {
        let msg = ClientMsg::<MsgClientMarketingMessageUpdate2>::from_packet(packet).ok()?;
        let mut r = Reader::new(&msg.payload);
        let mut messages = Vec::with_capacity((msg.body.count as usize).min(64));
        for _ in 0..msg.body.count {
            let len = r.i32().ok()?.checked_sub(4)? as usize;
            let mut item = Reader::new(r.bytes(len).ok()?);
            messages.push(MarketingMessage {
                id: GlobalId(item.u64().ok()?),
                url: item.cstring().ok()?,
                flags: EMarketingMessageFlags(item.i32().ok()?),
            });
        }
        Some(Self {
            update_time: UNIX_EPOCH + Duration::from_secs(msg.body.marketing_message_update_time as u64),
            messages,
        })
    }
}
