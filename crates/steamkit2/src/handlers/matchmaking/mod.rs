//! Lobbies (SteamKit2's `SteamMatchmaking`). Lobbies seen in responses and
//! updates are cached; read them with [`SteamClient::lobby`].

mod lobby;

pub use lobby::{Filter, JoinLobby, Lobby, LobbyCache, Member, Metadata};

use prost::Message;
use steamkit2_protos::steam::{
    CMsgClientMmsCreateLobby, CMsgClientMmsCreateLobbyResponse, CMsgClientMmsGetLobbyData, CMsgClientMmsGetLobbyList,
    CMsgClientMmsGetLobbyListResponse, CMsgClientMmsInviteToLobby, CMsgClientMmsJoinLobby,
    CMsgClientMmsJoinLobbyResponse, CMsgClientMmsLeaveLobby, CMsgClientMmsLeaveLobbyResponse, CMsgClientMmsLobbyData,
    CMsgClientMmsSetLobbyData, CMsgClientMmsSetLobbyDataResponse, CMsgClientMmsSetLobbyOwner,
    CMsgClientMmsSetLobbyOwnerResponse, CMsgClientMmsUserJoinedLobby, CMsgClientMmsUserLeftLobby,
};

use crate::client::SteamClient;
use crate::enums::{EChatRoomEnterResponse, ELobbyType, EMsg, EResult};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};
use crate::types::SteamId;
use lobby::{encode_metadata, lobby_data, Metadata as Meta};

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamMatchmaking`.
    pub enum MatchmakingCallback {
        /// `ClientMMSCreateLobbyResponse`.
        CreateLobby(CMsgClientMmsCreateLobbyResponse) = ClientMMSCreateLobbyResponse,
        /// `ClientMMSSetLobbyDataResponse`.
        SetLobbyData(CMsgClientMmsSetLobbyDataResponse) = ClientMMSSetLobbyDataResponse,
        /// `ClientMMSSetLobbyOwnerResponse`.
        SetLobbyOwner(CMsgClientMmsSetLobbyOwnerResponse) = ClientMMSSetLobbyOwnerResponse,
        /// `ClientMMSGetLobbyListResponse`.
        LobbyList(CMsgClientMmsGetLobbyListResponse) = ClientMMSGetLobbyListResponse,
        /// `ClientMMSJoinLobbyResponse`.
        JoinLobby(CMsgClientMmsJoinLobbyResponse) = ClientMMSJoinLobbyResponse,
        /// `ClientMMSLeaveLobbyResponse`.
        LeaveLobby(CMsgClientMmsLeaveLobbyResponse) = ClientMMSLeaveLobbyResponse,
        /// `ClientMMSLobbyData`: a lobby changed.
        LobbyData(CMsgClientMmsLobbyData) = ClientMMSLobbyData,
        /// `ClientMMSUserJoinedLobby`.
        UserJoinedLobby(CMsgClientMmsUserJoinedLobby) = ClientMMSUserJoinedLobby,
        /// `ClientMMSUserLeftLobby`.
        UserLeftLobby(CMsgClientMmsUserLeftLobby) = ClientMMSUserLeftLobby,
    }
}

impl MatchmakingCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

fn check(result: i32) -> Result<()> {
    match EResult(result) {
        EResult::OK => Ok(()),
        other => Err(Error::Steam(other)),
    }
}

fn routed<M: Message>(emsg: EMsg, app_id: u32, body: M) -> ProtoMsg<M> {
    let mut msg = ProtoMsg::new(emsg, body);
    msg.header.routing_appid = Some(app_id);
    msg
}

impl SteamClient {
    fn lobby_cache(&self) -> &LobbyCache {
        &self.handler_state().lobbies
    }

    /// A cached lobby.
    pub fn lobby(&self, app_id: u32, lobby: SteamId) -> Option<Lobby> {
        self.lobby_cache().get(app_id, lobby)
    }

    /// Creates a lobby and returns its ID. Needs a logged on session.
    pub async fn create_lobby(
        &self,
        app_id: u32,
        lobby_type: ELobbyType,
        max_members: i32,
        lobby_flags: i32,
        metadata: &Meta,
    ) -> Result<SteamId> {
        let cell_id = self.cell_id().ok_or(Error::NotConnected)?;
        let persona_name = self.persona_name().unwrap_or_default();
        let response: CMsgClientMmsCreateLobbyResponse = self
            .call(routed(
                EMsg::ClientMMSCreateLobby,
                app_id,
                CMsgClientMmsCreateLobby {
                    app_id: Some(app_id),
                    lobby_type: Some(lobby_type.0),
                    max_members: Some(max_members),
                    lobby_flags: Some(lobby_flags),
                    cell_id: Some(cell_id),
                    public_ip: self.public_ip(),
                    persona_name_owner: Some(persona_name.clone()),
                    metadata: encode_metadata(metadata),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        let me = self.steam_id().unwrap_or_default();
        let id = SteamId::from_u64(response.steam_id_lobby());
        self.lobby_cache().insert(
            app_id,
            Lobby {
                steam_id: id,
                lobby_type,
                lobby_flags,
                owner: Some(me),
                metadata: metadata.clone(),
                max_members,
                num_members: 1,
                members: vec![Member { steam_id: me, persona_name, metadata: Meta::new() }],
                distance: None,
                weight: None,
            },
        );
        Ok(id)
    }

    /// Updates a lobby we own.
    pub async fn set_lobby_data(
        &self,
        app_id: u32,
        lobby: SteamId,
        lobby_type: ELobbyType,
        max_members: i32,
        lobby_flags: i32,
        metadata: &Meta,
    ) -> Result<()> {
        let response: CMsgClientMmsSetLobbyDataResponse = self
            .call(routed(
                EMsg::ClientMMSSetLobbyData,
                app_id,
                CMsgClientMmsSetLobbyData {
                    app_id: Some(app_id),
                    steam_id_lobby: Some(lobby.as_u64()),
                    steam_id_member: Some(0),
                    lobby_type: Some(lobby_type.0),
                    max_members: Some(max_members),
                    lobby_flags: Some(lobby_flags),
                    metadata: encode_metadata(metadata),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        self.lobby_cache().update(app_id, lobby, |l| {
            l.lobby_type = lobby_type;
            l.lobby_flags = lobby_flags;
            l.max_members = max_members;
            l.metadata = metadata.clone();
        });
        Ok(())
    }

    /// Sets our own member metadata in a lobby.
    pub async fn set_lobby_member_data(&self, app_id: u32, lobby: SteamId, metadata: &Meta) -> Result<()> {
        let me = self.steam_id().ok_or(Error::NotConnected)?;
        let response: CMsgClientMmsSetLobbyDataResponse = self
            .call(routed(
                EMsg::ClientMMSSetLobbyData,
                app_id,
                CMsgClientMmsSetLobbyData {
                    app_id: Some(app_id),
                    steam_id_lobby: Some(lobby.as_u64()),
                    steam_id_member: Some(me.as_u64()),
                    metadata: encode_metadata(metadata),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        self.lobby_cache().update(app_id, lobby, |l| {
            for m in l.members.iter_mut().filter(|m| m.steam_id == me) {
                m.metadata = metadata.clone();
            }
        });
        Ok(())
    }

    /// Hands lobby ownership to another member.
    pub async fn set_lobby_owner(&self, app_id: u32, lobby: SteamId, new_owner: SteamId) -> Result<()> {
        let response: CMsgClientMmsSetLobbyOwnerResponse = self
            .call(routed(
                EMsg::ClientMMSSetLobbyOwner,
                app_id,
                CMsgClientMmsSetLobbyOwner {
                    app_id: Some(app_id),
                    steam_id_lobby: Some(lobby.as_u64()),
                    steam_id_new_owner: Some(new_owner.as_u64()),
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        self.lobby_cache().update(app_id, lobby, |l| l.owner = Some(new_owner));
        Ok(())
    }

    /// Lists lobbies of an app. `max_lobbies` of -1 means no limit.
    pub async fn get_lobby_list(&self, app_id: u32, filters: &[Filter], max_lobbies: i32) -> Result<Vec<Lobby>> {
        let cell_id = self.cell_id().ok_or(Error::NotConnected)?;
        let response: CMsgClientMmsGetLobbyListResponse = self
            .call(routed(
                EMsg::ClientMMSGetLobbyList,
                app_id,
                CMsgClientMmsGetLobbyList {
                    app_id: Some(app_id),
                    cell_id: Some(cell_id),
                    public_ip: self.public_ip(),
                    num_lobbies_requested: Some(max_lobbies),
                    filters: filters.iter().map(Filter::serialize).collect(),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        // The cache was updated from this response before it got here.
        Ok(lobby::lobby_list(&response, self.lobby_cache()))
    }

    /// Joins a lobby.
    pub async fn join_lobby(&self, app_id: u32, lobby: SteamId) -> Result<JoinLobby> {
        let persona_name = self.persona_name().unwrap_or_default();
        let response: CMsgClientMmsJoinLobbyResponse = self
            .call(routed(
                EMsg::ClientMMSJoinLobby,
                app_id,
                CMsgClientMmsJoinLobby {
                    app_id: Some(app_id),
                    steam_id_lobby: Some(lobby.as_u64()),
                    persona_name: Some(persona_name),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        Ok(JoinLobby {
            app_id: response.app_id(),
            response: EChatRoomEnterResponse(response.chat_room_enter_response()),
            lobby: response.steam_id_lobby.and_then(|id| self.lobby(app_id, SteamId::from_u64(id))),
        })
    }

    /// Leaves a lobby.
    pub async fn leave_lobby(&self, app_id: u32, lobby: SteamId) -> Result<()> {
        let response: CMsgClientMmsLeaveLobbyResponse = self
            .call(routed(
                EMsg::ClientMMSLeaveLobby,
                app_id,
                CMsgClientMmsLeaveLobby { app_id: Some(app_id), steam_id_lobby: Some(lobby.as_u64()) },
            ))
            .await?
            .decode()?;
        check(response.eresult())
    }

    /// Requests a lobby's data.
    pub async fn get_lobby_data(&self, app_id: u32, lobby: SteamId) -> Result<Lobby> {
        let response: CMsgClientMmsLobbyData = self
            .call(routed(
                EMsg::ClientMMSGetLobbyData,
                app_id,
                CMsgClientMmsGetLobbyData { app_id: Some(app_id), steam_id_lobby: Some(lobby.as_u64()) },
            ))
            .await?
            .decode()?;
        Ok(lobby_data(&response, self.lobby_cache()))
    }

    /// Invites a user to a lobby.
    pub fn invite_to_lobby(&self, app_id: u32, lobby: SteamId, user: SteamId) -> Result<()> {
        self.send(routed(
            EMsg::ClientMMSInviteToLobby,
            app_id,
            CMsgClientMmsInviteToLobby {
                app_id: Some(app_id),
                steam_id_lobby: Some(lobby.as_u64()),
                steam_id_user_invited: Some(user.as_u64()),
            },
        ))
    }
}
