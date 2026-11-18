//! The friends cache (SteamKit2's `AccountCache`), kept up to date from
//! `ClientFriendsList`, `ClientPersonaState`, `ClientAccountInfo` and
//! `ClientPersonaChangeResponse`.

use std::collections::HashMap;
use std::sync::Mutex;

use steamwire_protos::steam::{
    CMsgClientAccountInfo, CMsgClientFriendsList, CMsgClientPersonaState, CMsgClientRequestFriendData,
    CMsgPersonaChangeResponse,
};

use crate::client::SteamClient;
use crate::enums::{
    EClanRelationship, EClientPersonaStateFlag, EFriendRelationship, EMsg, EPersonaState, EPersonaStateFlag,
};
use crate::messages::{Packet, ProtoMsg};
use crate::types::{GameId, SteamId};

/// What's known about a user.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Friend {
    /// The user.
    pub steam_id: SteamId,
    /// The persona name.
    pub name: Option<String>,
    /// Online, away, ...
    pub persona_state: EPersonaState,
    /// Persona flags.
    pub persona_state_flags: EPersonaStateFlag,
    /// Our relationship.
    pub relationship: EFriendRelationship,
    /// The game being played, by name (non-Steam games).
    pub game_name: Option<String>,
    /// The game being played.
    pub game_id: GameId,
    /// The app being played.
    pub game_app_id: u32,
    /// The avatar hash (SHA-1).
    pub avatar_hash: Vec<u8>,
}

impl Friend {
    /// The avatar URL, if the avatar is known.
    pub fn avatar_url(&self) -> Option<String> {
        avatar_url(&self.avatar_hash)
    }
}

/// The URL of the full-size avatar for a hash.
pub fn avatar_url(hash: &[u8]) -> Option<String> {
    if hash.is_empty() || hash.iter().all(|&b| b == 0) {
        return None;
    }
    Some(format!("https://avatars.steamstatic.com/{}_full.jpg", hex::encode(hash)))
}

/// What's known about a clan (group).
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Clan {
    /// The clan.
    pub steam_id: SteamId,
    /// The name.
    pub name: Option<String>,
    /// Our relationship.
    pub relationship: EClanRelationship,
    /// The avatar hash.
    pub avatar_hash: Vec<u8>,
}

#[derive(Debug, Default)]
struct Inner {
    local_name: Option<String>,
    local_state: EPersonaState,
    friends: Vec<SteamId>,
    clans: Vec<SteamId>,
    users: HashMap<SteamId, Friend>,
    clan_info: HashMap<SteamId, Clan>,
}

/// The cache. One per [`SteamClient`].
#[derive(Debug, Default)]
pub struct FriendsCache {
    inner: Mutex<Inner>,
}

impl Inner {
    fn user(&mut self, id: SteamId) -> &mut Friend {
        self.users.entry(id).or_insert_with(|| Friend { steam_id: id, ..Default::default() })
    }

    fn clan(&mut self, id: SteamId) -> &mut Clan {
        self.clan_info.entry(id).or_insert_with(|| Clan { steam_id: id, ..Default::default() })
    }
}

impl FriendsCache {
    pub(crate) fn observe(&self, client: &SteamClient, packet: &Packet) {
        match packet.emsg {
            EMsg::ClientAccountInfo => {
                if let Ok(info) = packet.decode::<CMsgClientAccountInfo>() {
                    self.inner.lock().unwrap().local_name = info.persona_name;
                }
            }
            EMsg::ClientPersonaChangeResponse => {
                if let Ok(resp) = packet.decode::<CMsgPersonaChangeResponse>() {
                    self.inner.lock().unwrap().local_name = resp.player_name;
                }
            }
            EMsg::ClientFriendsList => {
                if let Ok(list) = packet.decode::<CMsgClientFriendsList>() {
                    self.handle_friends_list(client, list);
                }
            }
            EMsg::ClientPersonaState => {
                if let Ok(state) = packet.decode::<CMsgClientPersonaState>() {
                    self.handle_persona_state(client.steam_id(), state);
                }
            }
            _ => {}
        }
    }

    fn handle_friends_list(&self, client: &SteamClient, list: CMsgClientFriendsList) {
        let incremental = list.bincremental();
        let mut request = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            if !incremental {
                inner.friends.clear();
                inner.clans.clear();
            }
            for friend in &list.friends {
                let id = SteamId::from_u64(friend.ulfriendid());
                let relationship = friend.efriendrelationship() as i32;
                if id.is_individual_account() {
                    let rel = EFriendRelationship(relationship);
                    inner.user(id).relationship = rel;
                    if inner.friends.contains(&id) {
                        if rel == EFriendRelationship::None {
                            inner.friends.retain(|f| *f != id);
                        }
                    } else {
                        inner.friends.push(id);
                    }
                } else if id.is_clan_account() {
                    let rel = EClanRelationship(relationship);
                    inner.clan(id).relationship = rel;
                    if inner.clans.contains(&id) {
                        if rel == EClanRelationship::None || rel == EClanRelationship::Kicked {
                            inner.clans.retain(|c| *c != id);
                        }
                    } else {
                        inner.clans.push(id);
                    }
                }
                if !incremental {
                    request.push(id.as_u64());
                }
            }
        }
        // Steam only sends persona info for online friends; ask for the rest.
        if !request.is_empty() {
            let _ = client.send(ProtoMsg::new(
                EMsg::ClientRequestFriendData,
                CMsgClientRequestFriendData {
                    persona_state_requested: Some(client.default_persona_state_flags().0 as u32),
                    friends: request,
                },
            ));
        }
    }

    fn handle_persona_state(&self, local: Option<SteamId>, state: CMsgClientPersonaState) {
        let flags = EClientPersonaStateFlag(state.status_flags() as i32);
        let mut inner = self.inner.lock().unwrap();
        for friend in &state.friends {
            let id = SteamId::from_u64(friend.friendid());
            if Some(id) == local {
                // SteamKit2's cache returns its LocalUser for our own ID.
                if flags.contains(EClientPersonaStateFlag::PlayerName) {
                    inner.local_name.clone_from(&friend.player_name);
                }
                if flags.contains(EClientPersonaStateFlag::Presence) {
                    inner.local_state = EPersonaState(friend.persona_state() as i32);
                }
            } else if id.is_individual_account() {
                let user = inner.user(id);
                if flags.contains(EClientPersonaStateFlag::PlayerName) {
                    user.name.clone_from(&friend.player_name);
                }
                if flags.contains(EClientPersonaStateFlag::Presence) {
                    user.avatar_hash = friend.avatar_hash.clone().unwrap_or_default();
                    user.persona_state = EPersonaState(friend.persona_state() as i32);
                    user.persona_state_flags = EPersonaStateFlag(friend.persona_state_flags() as i32);
                }
                if flags.contains(EClientPersonaStateFlag::GameDataBlob) {
                    user.game_name.clone_from(&friend.game_name);
                    user.game_id = GameId::from_u64(friend.gameid());
                    user.game_app_id = friend.game_played_app_id();
                }
            } else if id.is_clan_account() {
                let clan = inner.clan(id);
                if flags.contains(EClientPersonaStateFlag::PlayerName) {
                    clan.name.clone_from(&friend.player_name);
                }
                if flags.contains(EClientPersonaStateFlag::Presence) {
                    clan.avatar_hash = friend.avatar_hash.clone().unwrap_or_default();
                }
            }
        }
    }

    pub(crate) fn set_local_name(&self, name: &str) {
        self.inner.lock().unwrap().local_name = Some(name.to_string());
    }

    pub(crate) fn set_local_state(&self, state: EPersonaState) {
        self.inner.lock().unwrap().local_state = state;
    }

    pub(crate) fn local_state(&self) -> EPersonaState {
        self.inner.lock().unwrap().local_state
    }
}

impl SteamClient {
    fn friends_cache(&self) -> &FriendsCache {
        &self.handler_state().friends
    }

    /// Our persona name, once known.
    pub fn persona_name(&self) -> Option<String> {
        self.friends_cache().inner.lock().unwrap().local_name.clone()
    }

    /// Our persona state as last set.
    pub fn persona_state(&self) -> EPersonaState {
        self.friends_cache().local_state()
    }

    /// The friends list.
    pub fn friends(&self) -> Vec<SteamId> {
        self.friends_cache().inner.lock().unwrap().friends.clone()
    }

    /// The clans (groups) list.
    pub fn clans(&self) -> Vec<SteamId> {
        self.friends_cache().inner.lock().unwrap().clans.clone()
    }

    /// What's known about a user (friend or not).
    pub fn friend(&self, steam_id: SteamId) -> Option<Friend> {
        self.friends_cache().inner.lock().unwrap().users.get(&steam_id).cloned()
    }

    /// What's known about a clan.
    pub fn clan(&self, steam_id: SteamId) -> Option<Clan> {
        self.friends_cache().inner.lock().unwrap().clan_info.get(&steam_id).cloned()
    }
}
