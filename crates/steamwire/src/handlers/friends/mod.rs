//! Friends, clans, persona state and legacy chat rooms (SteamKit2's
//! `SteamFriends`).
//!
//! The friends list and persona info are cached as messages arrive; read them
//! with [`SteamClient::friends`], [`SteamClient::friend`] and friends.

mod cache;
mod callbacks;

pub use cache::{avatar_url, Clan, Friend, FriendsCache};
pub use callbacks::*;

use crate::protos::steam::{
    CMsgClientAddFriend, CMsgClientChangeStatus, CMsgClientChatGetFriendMessageHistory,
    CMsgClientChatGetFriendMessageHistoryForOfflineMessages, CMsgClientChatInvite, CMsgClientFriendMsg,
    CMsgClientFriendProfileInfo, CMsgClientFriendProfileInfoResponse, CMsgClientRemoveFriend,
    CMsgClientRequestFriendData,
};

use crate::client::SteamClient;
use crate::enums::{
    EChatAction, EChatEntryType, EChatInfoType, EChatMemberStateChange, EClientPersonaStateFlag, EMsg, EPersonaState,
    EResult,
};
use crate::error::{Error, Result};
use crate::messages::generated::{
    MsgClientChatAction, MsgClientChatMemberInfo, MsgClientChatMsg, MsgClientJoinChat, MsgClientSetIgnoreFriend,
    MsgClientSetIgnoreFriendResponse,
};
use crate::messages::{ClientMsg, ProtoMsg};
use crate::types::SteamId;

impl SteamClient {
    /// Sets the persona name (keeping the current persona state).
    pub fn set_persona_name(&self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        self.handler_state().friends.set_local_name(&name);
        self.send(ProtoMsg::new(
            EMsg::ClientChangeStatus,
            CMsgClientChangeStatus {
                persona_state: Some(self.persona_state().0 as u32),
                player_name: Some(name),
                ..Default::default()
            },
        ))
    }

    /// Sets the persona state (online, away, invisible, ...).
    pub fn set_persona_state(&self, state: EPersonaState) -> Result<()> {
        self.handler_state().friends.set_local_state(state);
        self.send(ProtoMsg::new(
            EMsg::ClientChangeStatus,
            CMsgClientChangeStatus { persona_state: Some(state.0 as u32), ..Default::default() },
        ))
    }

    /// Sends a chat message to a friend.
    pub fn send_chat_message(&self, target: SteamId, entry_type: EChatEntryType, message: &str) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientFriendMsg,
            CMsgClientFriendMsg {
                steamid: Some(target.as_u64()),
                chat_entry_type: Some(entry_type.0),
                message: Some(message.as_bytes().to_vec()),
                ..Default::default()
            },
        ))
    }

    /// Sends a friend request by account name or email. Steam now answers
    /// these with `EResult::Fail` (`FriendsCallback::FriendAdded`); use
    /// [`add_friend`](Self::add_friend) with a Steam ID.
    pub fn add_friend_by_name(&self, account_name_or_email: &str) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientAddFriend,
            CMsgClientAddFriend {
                accountname_or_email_to_add: Some(account_name_or_email.into()),
                ..Default::default()
            },
        ))
    }

    /// Sends a friend request, or accepts one.
    pub fn add_friend(&self, steam_id: SteamId) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientAddFriend,
            CMsgClientAddFriend { steamid_to_add: Some(steam_id.as_u64()), ..Default::default() },
        ))
    }

    /// Removes a friend, or declines a friend request.
    pub fn remove_friend(&self, steam_id: SteamId) -> Result<()> {
        self.send(ProtoMsg::new(EMsg::ClientRemoveFriend, CMsgClientRemoveFriend { friendid: Some(steam_id.as_u64()) }))
    }

    /// Joins a chat room (a clan ID joins the clan's chat).
    pub fn join_chat(&self, chat: SteamId) -> Result<()> {
        self.send(ClientMsg::new(MsgClientJoinChat { steam_id_chat: chat.as_chat_target(), is_voice_speaker: false }))
    }

    /// Leaves a chat room.
    pub fn leave_chat(&self, chat: SteamId) -> Result<()> {
        let me = self.steam_id().unwrap_or_default();
        let mut msg = ClientMsg::new(MsgClientChatMemberInfo {
            steam_id_chat: chat.as_chat_target(),
            r#type: EChatInfoType::StateChange,
        });
        msg.payload.extend_from_slice(&me.as_u64().to_le_bytes());
        msg.payload.extend_from_slice(&(EChatMemberStateChange::Left.0 as u32).to_le_bytes());
        msg.payload.extend_from_slice(&me.as_u64().to_le_bytes());
        self.send(msg)
    }

    /// Sends a message to a chat room.
    pub fn send_chat_room_message(&self, chat: SteamId, entry_type: EChatEntryType, message: &str) -> Result<()> {
        let mut msg = ClientMsg::new(MsgClientChatMsg {
            steam_id_chatter: self.steam_id().unwrap_or_default(),
            steam_id_chat_room: chat.as_chat_target(),
            chat_msg_type: entry_type,
        });
        msg.payload.extend_from_slice(message.as_bytes());
        msg.payload.push(0);
        self.send(msg)
    }

    /// Invites a user to a chat room.
    pub fn invite_user_to_chat(&self, user: SteamId, chat: SteamId) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientChatInvite,
            CMsgClientChatInvite {
                steam_id_chat: Some(chat.as_chat_target().as_u64()),
                steam_id_invited: Some(user.as_u64()),
                steam_id_patron: Some(self.steam_id().unwrap_or_default().as_u64()),
                ..Default::default()
            },
        ))
    }

    fn chat_action(&self, chat: SteamId, member: SteamId, action: EChatAction) -> Result<()> {
        self.send(ClientMsg::new(MsgClientChatAction {
            steam_id_chat: chat.as_chat_target(),
            steam_id_user_to_act_on: member,
            chat_action: action,
        }))
    }

    /// Kicks a member from a chat room.
    pub fn kick_chat_member(&self, chat: SteamId, member: SteamId) -> Result<()> {
        self.chat_action(chat, member, EChatAction::Kick)
    }

    /// Bans a member from a chat room.
    pub fn ban_chat_member(&self, chat: SteamId, member: SteamId) -> Result<()> {
        self.chat_action(chat, member, EChatAction::Ban)
    }

    /// Unbans a member of a chat room.
    pub fn unban_chat_member(&self, chat: SteamId, member: SteamId) -> Result<()> {
        self.chat_action(chat, member, EChatAction::UnBan)
    }

    /// Requests persona info for users or clans. `None` uses
    /// [`Config::persona_state_flags`](crate::Config::persona_state_flags).
    /// Answers arrive as [`FriendsCallback::PersonaState`].
    pub fn request_friend_info(&self, ids: &[SteamId], flags: Option<EClientPersonaStateFlag>) -> Result<()> {
        let flags = flags.unwrap_or_else(|| self.default_persona_state_flags());
        self.send(ProtoMsg::new(
            EMsg::ClientRequestFriendData,
            CMsgClientRequestFriendData {
                persona_state_requested: Some(flags.0 as u32),
                friends: ids.iter().map(|id| id.as_u64()).collect(),
            },
        ))
    }

    /// Ignores (blocks) or unignores a user.
    pub async fn ignore_friend(&self, steam_id: SteamId, ignore: bool) -> Result<()> {
        let packet = self
            .call(ClientMsg::new(MsgClientSetIgnoreFriend {
                my_steam_id: self.steam_id().unwrap_or_default(),
                steam_id_friend: steam_id,
                ignore: ignore as u8,
            }))
            .await?;
        let msg = ClientMsg::<MsgClientSetIgnoreFriendResponse>::from_packet(&packet)?;
        match msg.body.result {
            EResult::OK => Ok(()),
            other => Err(Error::Steam(other)),
        }
    }

    /// Requests a user's profile info.
    pub async fn request_profile_info(&self, steam_id: SteamId) -> Result<CMsgClientFriendProfileInfoResponse> {
        let response: CMsgClientFriendProfileInfoResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientFriendProfileInfo,
                CMsgClientFriendProfileInfo { steamid_friend: Some(steam_id.as_u64()) },
            ))
            .await?
            .decode()?;
        match EResult(response.eresult()) {
            EResult::OK => Ok(response),
            other => Err(Error::Steam(other)),
        }
    }

    /// Requests the recent message history with a friend. The answer arrives
    /// as [`FriendsCallback::MessageHistory`].
    pub fn request_message_history(&self, steam_id: SteamId) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientChatGetFriendMessageHistory,
            CMsgClientChatGetFriendMessageHistory { steamid: Some(steam_id.as_u64()) },
        ))
    }

    /// Requests messages received while offline, as
    /// [`FriendsCallback::MessageHistory`] callbacks.
    pub fn request_offline_messages(&self) -> Result<()> {
        self.send(ProtoMsg::new(
            EMsg::ClientChatGetFriendMessageHistoryForOfflineMessages,
            CMsgClientChatGetFriendMessageHistoryForOfflineMessages::default(),
        ))
    }
}
