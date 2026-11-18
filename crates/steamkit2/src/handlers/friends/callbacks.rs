use steamkit2_protos::steam::{
    c_msg_client_persona_state, CMsgClientAddFriendResponse, CMsgClientChatGetFriendMessageHistoryResponse,
    CMsgClientChatInvite, CMsgClientClanState, CMsgClientFriendMsgIncoming, CMsgClientFriendProfileInfoResponse,
    CMsgClientFriendsList, CMsgClientPersonaState, CMsgPersonaChangeResponse,
};

use crate::enums::{
    EAccountType, EChatAction, EChatActionResult, EChatInfoType, EChatMemberStateChange, EChatRoomEnterResponse,
    EChatRoomType, EClientPersonaStateFlag, EMsg, EResult, EUniverse,
};
use crate::handlers::proto_callbacks;
use crate::messages::generated::{
    MsgClientChatActionResult, MsgClientChatEnter, MsgClientChatMemberInfo, MsgClientChatMsg, MsgClientChatRoomInfo,
    MsgClientSetIgnoreFriendResponse,
};
use crate::messages::{ClientMsg, Packet, Reader};
use crate::types::kv::KeyValue;
use crate::types::SteamId;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamFriends`.
    pub enum FriendsCallback {
        /// `ClientFriendsList`: the full list after logon, then incremental changes.
        FriendsList(CMsgClientFriendsList) = ClientFriendsList,
        /// `ClientClanState`.
        ClanState(CMsgClientClanState) = ClientClanState,
        /// `ClientFriendMsgIncoming`: a chat message from a friend.
        FriendMsg(CMsgClientFriendMsgIncoming) = ClientFriendMsgIncoming,
        /// `ClientFriendMsgEchoToSender`: a message we sent from another session.
        FriendMsgEcho(CMsgClientFriendMsgIncoming) = ClientFriendMsgEchoToSender,
        /// `ClientAddFriendResponse`.
        FriendAdded(CMsgClientAddFriendResponse) = ClientAddFriendResponse,
        /// `ClientChatInvite`.
        ChatInvite(CMsgClientChatInvite) = ClientChatInvite,
        /// `ClientFriendProfileInfoResponse`.
        ProfileInfo(CMsgClientFriendProfileInfoResponse) = ClientFriendProfileInfoResponse,
        /// `ClientPersonaChangeResponse`.
        PersonaChange(CMsgPersonaChangeResponse) = ClientPersonaChangeResponse,
    }
    extra {
        /// `ClientPersonaState`: one entry per user or clan, with the
        /// flags saying which fields are set.
        PersonaState(PersonaState),
        /// `ClientChatGetFriendMessageHistoryResponse`.
        MessageHistory(FriendMessageHistory),
        /// `ClientChatEnter`.
        ChatEnter(ChatEnter),
        /// `ClientChatMsg`.
        ChatMsg(ChatMsg),
        /// `ClientChatMemberInfo`.
        ChatMemberInfo(ChatMemberInfo),
        /// `ClientChatRoomInfo`.
        ChatRoomInfo(ChatRoomInfo),
        /// `ClientChatActionResult`.
        ChatActionResult(ChatActionResult),
        /// `ClientSetIgnoreFriendResponse`.
        IgnoreFriend(IgnoreFriend),
    }
}

impl FriendsCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Some(match packet.emsg {
            EMsg::ClientPersonaState => {
                let msg: CMsgClientPersonaState = packet.decode().ok()?;
                Self::PersonaState(PersonaState {
                    flags: EClientPersonaStateFlag(msg.status_flags() as i32),
                    friends: msg.friends,
                })
            }
            EMsg::ClientChatGetFriendMessageHistoryResponse => {
                let msg: CMsgClientChatGetFriendMessageHistoryResponse = packet.decode().ok()?;
                let universe = SteamId::from_u64(msg.steamid()).universe();
                Self::MessageHistory(FriendMessageHistory::new(msg, universe))
            }
            EMsg::ClientChatEnter => Self::ChatEnter(ChatEnter::parse(packet)?),
            EMsg::ClientChatMsg => {
                let msg = ClientMsg::<MsgClientChatMsg>::from_packet(packet).ok()?;
                let text = msg.payload.strip_suffix(&[0]).unwrap_or(&msg.payload);
                let end = text.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
                Self::ChatMsg(ChatMsg {
                    chatter: msg.body.steam_id_chatter,
                    chat_room: msg.body.steam_id_chat_room,
                    entry_type: msg.body.chat_msg_type,
                    message: String::from_utf8_lossy(&text[..end]).into_owned(),
                })
            }
            EMsg::ClientChatMemberInfo => {
                let msg = ClientMsg::<MsgClientChatMemberInfo>::from_packet(packet).ok()?;
                let state_change =
                    (msg.body.r#type == EChatInfoType::StateChange).then(|| StateChange::parse(&msg.payload)).flatten();
                Self::ChatMemberInfo(ChatMemberInfo {
                    chat_room: msg.body.steam_id_chat,
                    info_type: msg.body.r#type,
                    state_change,
                })
            }
            EMsg::ClientChatRoomInfo => {
                let msg = ClientMsg::<MsgClientChatRoomInfo>::from_packet(packet).ok()?;
                Self::ChatRoomInfo(ChatRoomInfo { chat_room: msg.body.steam_id_chat, info_type: msg.body.r#type })
            }
            EMsg::ClientChatActionResult => {
                let msg = ClientMsg::<MsgClientChatActionResult>::from_packet(packet).ok()?;
                Self::ChatActionResult(ChatActionResult {
                    chat_room: msg.body.steam_id_chat,
                    chatter: msg.body.steam_id_user_acted_on,
                    action: msg.body.chat_action,
                    result: msg.body.action_result,
                })
            }
            EMsg::ClientSetIgnoreFriendResponse => {
                let msg = ClientMsg::<MsgClientSetIgnoreFriendResponse>::from_packet(packet).ok()?;
                Self::IgnoreFriend(IgnoreFriend { friend: msg.body.friend_id, result: msg.body.result })
            }
            _ => return Self::decode_proto(packet),
        })
    }
}

/// A batch of persona updates.
#[derive(Clone, Debug)]
pub struct PersonaState {
    /// Which fields the entries carry.
    pub flags: EClientPersonaStateFlag,
    /// The entries.
    pub friends: Vec<c_msg_client_persona_state::Friend>,
}

/// A message in a friend's chat history.
#[derive(Clone, Debug)]
pub struct FriendMessage {
    /// Who sent it.
    pub sender: SteamId,
    /// Unread by us.
    pub unread: bool,
    /// The text.
    pub message: String,
    /// Unix time.
    pub timestamp: u32,
}

/// Recent chat history with a friend.
#[derive(Clone, Debug)]
pub struct FriendMessageHistory {
    /// The result.
    pub result: EResult,
    /// The friend.
    pub steam_id: SteamId,
    /// The messages.
    pub messages: Vec<FriendMessage>,
}

impl FriendMessageHistory {
    fn new(msg: CMsgClientChatGetFriendMessageHistoryResponse, universe: EUniverse) -> Self {
        let universe = if universe == EUniverse::Invalid { EUniverse::Public } else { universe };
        Self {
            result: EResult(msg.success() as i32),
            steam_id: SteamId::from_u64(msg.steamid()),
            messages: msg
                .messages
                .into_iter()
                .map(|m| FriendMessage {
                    sender: SteamId::new(m.accountid(), 1, universe, EAccountType::Individual),
                    unread: m.unread(),
                    timestamp: m.timestamp(),
                    message: m.message.unwrap_or_default(),
                })
                .collect(),
        }
    }
}

/// A chat room member, as sent in binary KeyValues.
#[derive(Clone, Debug)]
pub struct ChatMember {
    /// The raw KeyValues (`SteamID`, `Permissions`, `Details`).
    pub key_values: KeyValue,
}

impl ChatMember {
    /// The member.
    pub fn steam_id(&self) -> SteamId {
        SteamId::from_u64(self.key_values["SteamID"].as_u64().unwrap_or(0))
    }

    /// Clan permissions (`EClanPermission` bits).
    pub fn details(&self) -> i32 {
        self.key_values["Details"].as_i32().unwrap_or(0)
    }

    /// Chat permissions (`EChatPermission` bits).
    pub fn permissions(&self) -> i32 {
        self.key_values["Permissions"].as_i32().unwrap_or(0)
    }

    fn read(data: &[u8]) -> Option<(Self, usize)> {
        let (kv, used) = KeyValue::parse_binary_prefix(data).ok()?;
        // MessageObject wraps its fields in a "MessageObject" node.
        let key_values = kv.children().first().cloned().unwrap_or(kv);
        Some((Self { key_values }, used))
    }
}

/// The result of joining a chat room.
#[derive(Clone, Debug)]
pub struct ChatEnter {
    /// The room.
    pub chat_room: SteamId,
    /// The friend, for friend chats.
    pub friend: SteamId,
    /// The kind of room.
    pub room_type: EChatRoomType,
    /// The owner.
    pub owner: SteamId,
    /// The clan, for clan chats.
    pub clan: SteamId,
    /// Room flags.
    pub chat_flags: u8,
    /// Whether we got in.
    pub enter_response: EChatRoomEnterResponse,
    /// The member count.
    pub num_members: i32,
    /// The room name.
    pub name: String,
    /// The members (only on success).
    pub members: Vec<ChatMember>,
}

impl ChatEnter {
    fn parse(packet: &Packet) -> Option<Self> {
        let msg = ClientMsg::<MsgClientChatEnter>::from_packet(packet).ok()?;
        let b = &msg.body;
        let mut r = Reader::new(&msg.payload);
        let name = r.cstring().unwrap_or_default();
        let mut members = Vec::new();
        if b.enter_response == EChatRoomEnterResponse::Success {
            let mut rest = r.remaining();
            for _ in 0..b.num_members.max(0) {
                let Some((member, used)) = ChatMember::read(rest) else { break };
                members.push(member);
                rest = &rest[used..];
            }
        }
        Some(Self {
            chat_room: b.steam_id_chat,
            friend: b.steam_id_friend,
            room_type: b.chat_room_type,
            owner: b.steam_id_owner,
            clan: b.steam_id_clan,
            chat_flags: b.chat_flags,
            enter_response: b.enter_response,
            num_members: b.num_members,
            name,
            members,
        })
    }
}

/// A chat room message.
#[derive(Clone, Debug)]
pub struct ChatMsg {
    /// The sender.
    pub chatter: SteamId,
    /// The room.
    pub chat_room: SteamId,
    /// Message, typing, ...
    pub entry_type: crate::enums::EChatEntryType,
    /// The text.
    pub message: String,
}

/// A member entered, left, was kicked, ...
#[derive(Clone, Debug)]
pub struct StateChange {
    /// Who it happened to.
    pub acted_on: SteamId,
    /// What happened.
    pub state_change: EChatMemberStateChange,
    /// Who did it.
    pub acted_by: SteamId,
    /// The member's info, when they entered.
    pub member: Option<ChatMember>,
}

impl StateChange {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        let acted_on = SteamId::from_u64(r.u64().ok()?);
        let state_change = EChatMemberStateChange(r.i32().ok()?);
        let acted_by = SteamId::from_u64(r.u64().ok()?);
        let member = if state_change == EChatMemberStateChange::Entered {
            ChatMember::read(r.remaining()).map(|(m, _)| m)
        } else {
            None
        };
        Some(Self { acted_on, state_change, acted_by, member })
    }
}

/// `ClientChatMemberInfo`.
#[derive(Clone, Debug)]
pub struct ChatMemberInfo {
    /// The room.
    pub chat_room: SteamId,
    /// The kind of info.
    pub info_type: EChatInfoType,
    /// Set for [`EChatInfoType::StateChange`].
    pub state_change: Option<StateChange>,
}

/// `ClientChatRoomInfo`.
#[derive(Clone, Debug)]
pub struct ChatRoomInfo {
    /// The room.
    pub chat_room: SteamId,
    /// The kind of info.
    pub info_type: EChatInfoType,
}

/// The result of a kick, ban, ...
#[derive(Clone, Debug)]
pub struct ChatActionResult {
    /// The room.
    pub chat_room: SteamId,
    /// Who was acted on.
    pub chatter: SteamId,
    /// The action.
    pub action: EChatAction,
    /// The result.
    pub result: EChatActionResult,
}

/// The result of ignoring a friend.
#[derive(Clone, Debug)]
pub struct IgnoreFriend {
    /// The user.
    pub friend: SteamId,
    /// The result.
    pub result: EResult,
}
