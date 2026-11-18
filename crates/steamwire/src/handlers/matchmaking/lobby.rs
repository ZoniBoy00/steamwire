//! Lobbies, lobby list filters and the lobby cache.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use steamwire_protos::steam::{
    c_msg_client_mms_get_lobby_list, CMsgClientMmsGetLobbyListResponse, CMsgClientMmsJoinLobbyResponse,
    CMsgClientMmsLeaveLobbyResponse, CMsgClientMmsLobbyData, CMsgClientMmsUserJoinedLobby, CMsgClientMmsUserLeftLobby,
    MmsKeyValuePair,
};

use crate::enums::{
    EChatRoomEnterResponse, ELobbyComparison, ELobbyDistanceFilter, ELobbyFilterType, ELobbyType, EMsg, EResult,
};
use crate::messages::Packet;
use crate::types::kv::KeyValue;
use crate::types::SteamId;

/// Lobby or member metadata.
pub type Metadata = BTreeMap<String, String>;

/// A lobby member.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Member {
    /// The member.
    pub steam_id: SteamId,
    /// Their persona name.
    pub persona_name: String,
    /// Their metadata.
    pub metadata: Metadata,
}

/// A lobby (SteamKit2's `SteamMatchmaking.Lobby`).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Lobby {
    /// The lobby.
    pub steam_id: SteamId,
    /// Public, friends only, ...
    pub lobby_type: ELobbyType,
    /// Lobby flags.
    pub lobby_flags: i32,
    /// The owner, if known.
    pub owner: Option<SteamId>,
    /// Lobby metadata.
    pub metadata: Metadata,
    /// The member limit.
    pub max_members: i32,
    /// The member count.
    pub num_members: i32,
    /// The members; only known for lobbies we're in.
    pub members: Vec<Member>,
    /// Distance, from lobby list results.
    pub distance: Option<f32>,
    /// Weight, from lobby list results.
    pub weight: Option<i64>,
}

pub(crate) fn encode_metadata(metadata: &Metadata) -> Vec<MmsKeyValuePair> {
    metadata.iter().map(|(k, v)| MmsKeyValuePair { name: Some(k.clone()), value: Some(v.clone()) }).collect()
}

/// Decodes metadata from key/value pairs, or the legacy binary KeyValues buffer.
pub(crate) fn decode_metadata(pairs: &[MmsKeyValuePair], legacy: Option<&[u8]>) -> Metadata {
    if !pairs.is_empty() {
        return pairs.iter().filter_map(|p| Some((p.name.clone()?, p.value.clone().unwrap_or_default()))).collect();
    }
    let Some(buffer) = legacy.filter(|b| !b.is_empty()) else {
        return Metadata::new();
    };
    let Ok((kv, _)) = KeyValue::parse_binary_prefix(buffer) else {
        tracing::warn!("lobby metadata is of an unexpected format");
        return Metadata::new();
    };
    // `TryReadAsBinary` reads into the root node, so its children are the
    // first node's children.
    let node = kv.children().first().cloned().unwrap_or(kv);
    node.children().iter().filter_map(|c| Some((c.name.clone(), c.as_str()?.to_string()))).collect()
}

/// A lobby list filter.
#[derive(Clone, Debug)]
pub enum Filter {
    /// Only lobbies within a distance.
    Distance(ELobbyDistanceFilter),
    /// Sort by closeness of a numeric value.
    NearValue {
        /// The metadata key.
        key: String,
        /// The value.
        value: i32,
    },
    /// Compare a numeric value.
    Numerical {
        /// The metadata key.
        key: String,
        /// The comparison.
        comparison: ELobbyComparison,
        /// The value.
        value: i32,
    },
    /// Lobbies with at least this many free slots.
    SlotsAvailable(i32),
    /// Compare a string value.
    String {
        /// The metadata key.
        key: String,
        /// The comparison.
        comparison: ELobbyComparison,
        /// The value.
        value: String,
    },
}

impl Filter {
    pub(crate) fn serialize(&self) -> c_msg_client_mms_get_lobby_list::Filter {
        let (filter_type, key, comparison, value) = match self {
            Filter::Distance(d) => (ELobbyFilterType::Distance, "", ELobbyComparison::Equal, d.0.to_string()),
            Filter::NearValue { key, value } => {
                (ELobbyFilterType::NearValue, key.as_str(), ELobbyComparison::Equal, value.to_string())
            }
            Filter::Numerical { key, comparison, value } => {
                (ELobbyFilterType::Numerical, key.as_str(), *comparison, value.to_string())
            }
            Filter::SlotsAvailable(n) => (ELobbyFilterType::SlotsAvailable, "", ELobbyComparison::Equal, n.to_string()),
            Filter::String { key, comparison, value } => {
                (ELobbyFilterType::String, key.as_str(), *comparison, value.clone())
            }
        };
        c_msg_client_mms_get_lobby_list::Filter {
            key: Some(key.to_string()),
            value: Some(value),
            comparision: Some(comparison.0),
            filter_type: Some(filter_type.0),
        }
    }
}

/// Lobbies we know about, per app (SteamKit2's `LobbyCache`).
#[derive(Debug, Default)]
pub struct LobbyCache {
    lobbies: Mutex<HashMap<u32, HashMap<SteamId, Lobby>>>,
}

impl LobbyCache {
    /// A cached lobby.
    pub fn get(&self, app_id: u32, lobby: SteamId) -> Option<Lobby> {
        self.lobbies.lock().unwrap().get(&app_id)?.get(&lobby).cloned()
    }

    pub(crate) fn insert(&self, app_id: u32, lobby: Lobby) {
        self.lobbies.lock().unwrap().entry(app_id).or_default().insert(lobby.steam_id, lobby);
    }

    pub(crate) fn update(&self, app_id: u32, lobby: SteamId, f: impl FnOnce(&mut Lobby)) {
        if let Some(l) = self.lobbies.lock().unwrap().get_mut(&app_id).and_then(|m| m.get_mut(&lobby)) {
            f(l);
        }
    }

    fn clear_members(&self, app_id: u32, lobby: SteamId) {
        self.update(app_id, lobby, |l| {
            l.members.clear();
            l.owner = None;
        });
    }

    pub(crate) fn observe(&self, packet: &Packet) {
        match packet.emsg {
            EMsg::ClientMMSGetLobbyListResponse => {
                if let Ok(body) = packet.decode::<CMsgClientMmsGetLobbyListResponse>() {
                    for lobby in lobby_list(&body, self) {
                        self.insert(body.app_id(), lobby);
                    }
                }
            }
            EMsg::ClientMMSJoinLobbyResponse => {
                if let Ok(body) = packet.decode::<CMsgClientMmsJoinLobbyResponse>() {
                    if let Some(lobby) = joined_lobby(&body, self) {
                        self.insert(body.app_id(), lobby);
                    }
                }
            }
            EMsg::ClientMMSLeaveLobbyResponse => {
                if let Ok(body) = packet.decode::<CMsgClientMmsLeaveLobbyResponse>() {
                    if body.eresult() == EResult::OK.0 {
                        self.clear_members(body.app_id(), SteamId::from_u64(body.steam_id_lobby()));
                    }
                }
            }
            EMsg::ClientMMSLobbyData => {
                if let Ok(body) = packet.decode::<CMsgClientMmsLobbyData>() {
                    let lobby = lobby_data(&body, self);
                    self.insert(body.app_id(), lobby);
                }
            }
            EMsg::ClientMMSUserJoinedLobby => {
                if let Ok(body) = packet.decode::<CMsgClientMmsUserJoinedLobby>() {
                    let user = SteamId::from_u64(body.steam_id_user());
                    self.update(body.app_id(), SteamId::from_u64(body.steam_id_lobby()), |l| {
                        if !l.members.is_empty() && !l.members.iter().any(|m| m.steam_id == user) {
                            l.members.push(Member {
                                steam_id: user,
                                persona_name: body.persona_name().to_string(),
                                metadata: Metadata::new(),
                            });
                            l.num_members = l.members.len() as i32;
                        }
                    });
                }
            }
            EMsg::ClientMMSUserLeftLobby => {
                if let Ok(body) = packet.decode::<CMsgClientMmsUserLeftLobby>() {
                    let user = SteamId::from_u64(body.steam_id_user());
                    let me = packet.steam_id();
                    let lobby_id = SteamId::from_u64(body.steam_id_lobby());
                    if Some(user) == me {
                        self.clear_members(body.app_id(), lobby_id);
                    } else {
                        self.update(body.app_id(), lobby_id, |l| {
                            l.members.retain(|m| m.steam_id != user);
                            l.num_members = l.members.len() as i32;
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn lobby_list(body: &CMsgClientMmsGetLobbyListResponse, cache: &LobbyCache) -> Vec<Lobby> {
    body.lobbies
        .iter()
        .map(|l| {
            let id = SteamId::from_u64(l.steam_id());
            let existing = cache.get(body.app_id(), id);
            Lobby {
                steam_id: id,
                lobby_type: ELobbyType(l.lobby_type()),
                lobby_flags: l.lobby_flags(),
                owner: existing.as_ref().and_then(|e| e.owner),
                metadata: decode_metadata(&l.metadata, l.metadata_kv.as_deref()),
                max_members: l.max_members(),
                num_members: l.num_members(),
                members: existing.map(|e| e.members).unwrap_or_default(),
                distance: l.distance,
                weight: l.weight,
            }
        })
        .collect()
}

pub(crate) fn joined_lobby(body: &CMsgClientMmsJoinLobbyResponse, cache: &LobbyCache) -> Option<Lobby> {
    let id = SteamId::from_u64(body.steam_id_lobby?);
    let members: Vec<Member> = body
        .members
        .iter()
        .map(|m| Member {
            steam_id: SteamId::from_u64(m.steam_id()),
            persona_name: m.persona_name().to_string(),
            metadata: decode_metadata(&m.metadata, m.metadata_kv.as_deref()),
        })
        .collect();
    let cached = cache.get(body.app_id(), id);
    Some(Lobby {
        steam_id: id,
        lobby_type: ELobbyType(body.lobby_type()),
        lobby_flags: body.lobby_flags(),
        owner: Some(SteamId::from_u64(body.steam_id_owner())),
        metadata: decode_metadata(&body.metadata, body.metadata_kv.as_deref()),
        max_members: body.max_members(),
        num_members: members.len() as i32,
        members,
        distance: cached.as_ref().and_then(|c| c.distance),
        weight: cached.and_then(|c| c.weight),
    })
}

pub(crate) fn lobby_data(body: &CMsgClientMmsLobbyData, cache: &LobbyCache) -> Lobby {
    let id = SteamId::from_u64(body.steam_id_lobby());
    let cached = cache.get(body.app_id(), id);
    let members = if body.members.is_empty() {
        cached.as_ref().map(|c| c.members.clone()).unwrap_or_default()
    } else {
        body.members
            .iter()
            .map(|m| Member {
                steam_id: SteamId::from_u64(m.steam_id()),
                persona_name: m.persona_name().to_string(),
                metadata: decode_metadata(&m.metadata, m.metadata_kv.as_deref()),
            })
            .collect()
    };
    Lobby {
        steam_id: id,
        lobby_type: ELobbyType(body.lobby_type()),
        lobby_flags: body.lobby_flags(),
        owner: Some(SteamId::from_u64(body.steam_id_owner())),
        metadata: decode_metadata(&body.metadata, body.metadata_kv.as_deref()),
        max_members: body.max_members(),
        num_members: body.num_members(),
        members,
        distance: cached.as_ref().and_then(|c| c.distance),
        weight: cached.and_then(|c| c.weight),
    }
}

/// The result of joining a lobby.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JoinLobby {
    /// The app.
    pub app_id: u32,
    /// Whether we got in.
    pub response: EChatRoomEnterResponse,
    /// The lobby, when we got in.
    pub lobby: Option<Lobby>,
}
