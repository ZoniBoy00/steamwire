//! Player counts and leaderboards (SteamKit2's `SteamUserStats`).

use prost::Message;
use steamkit2_protos::steam::{
    CMsgClientLbsFindOrCreateLb, CMsgClientLbsFindOrCreateLbResponse, CMsgClientLbsGetLbEntries,
    CMsgClientLbsGetLbEntriesResponse, CMsgDpGetNumberOfCurrentPlayers, CMsgDpGetNumberOfCurrentPlayersResponse,
};

use crate::client::SteamClient;
use crate::enums::{ELeaderboardDataRequest, ELeaderboardDisplayType, ELeaderboardSortMethod, EMsg, EResult};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};
use crate::types::{SteamId, UgcHandle};

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamUserStats`.
    pub enum UserStatsCallback {
        /// `ClientGetNumberOfCurrentPlayersDPResponse`.
        NumberOfPlayers(CMsgDpGetNumberOfCurrentPlayersResponse) = ClientGetNumberOfCurrentPlayersDPResponse,
        /// `ClientLBSFindOrCreateLBResponse`.
        FindOrCreateLeaderboard(CMsgClientLbsFindOrCreateLbResponse) = ClientLBSFindOrCreateLBResponse,
        /// `ClientLBSGetLBEntriesResponse`.
        LeaderboardEntries(CMsgClientLbsGetLbEntriesResponse) = ClientLBSGetLBEntriesResponse,
    }
}

impl UserStatsCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

/// A leaderboard.
#[derive(Clone, Debug)]
pub struct Leaderboard {
    /// The leaderboard ID.
    pub id: i32,
    /// The number of entries.
    pub entry_count: i32,
    /// How it's sorted.
    pub sort_method: ELeaderboardSortMethod,
    /// How scores are shown.
    pub display_type: ELeaderboardDisplayType,
}

/// A leaderboard entry.
#[derive(Clone, Debug)]
pub struct LeaderboardEntry {
    /// The rank.
    pub global_rank: i32,
    /// The score.
    pub score: i32,
    /// The user.
    pub steam_id: SteamId,
    /// Attached UGC.
    pub ugc_id: UgcHandle,
    /// Extra details (game-defined).
    pub details: Vec<i32>,
}

fn check(result: i32) -> Result<()> {
    match EResult(result) {
        EResult::OK => Ok(()),
        other => Err(Error::Steam(other)),
    }
}

fn routed<M: Message>(emsg: EMsg, app_id: u32, body: M) -> ProtoMsg<M> {
    // routing_appid has to be set to receive a response.
    let mut msg = ProtoMsg::new(emsg, body);
    msg.header.routing_appid = Some(app_id);
    msg
}

impl SteamClient {
    /// The number of players currently in an app.
    pub async fn get_number_of_current_players(&self, app_id: u32) -> Result<u32> {
        let response: CMsgDpGetNumberOfCurrentPlayersResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientGetNumberOfCurrentPlayersDP,
                CMsgDpGetNumberOfCurrentPlayers { appid: Some(app_id) },
            ))
            .await?
            .decode()?;
        check(response.eresult())?;
        Ok(response.player_count() as u32)
    }

    async fn find_or_create_leaderboard(&self, body: CMsgClientLbsFindOrCreateLb) -> Result<Leaderboard> {
        let app_id = body.app_id();
        let response: CMsgClientLbsFindOrCreateLbResponse =
            self.call(routed(EMsg::ClientLBSFindOrCreateLB, app_id, body)).await?.decode()?;
        check(response.eresult())?;
        Ok(Leaderboard {
            id: response.leaderboard_id(),
            entry_count: response.leaderboard_entry_count(),
            sort_method: ELeaderboardSortMethod(response.leaderboard_sort_method()),
            display_type: ELeaderboardDisplayType(response.leaderboard_display_type()),
        })
    }

    /// Finds a leaderboard by name.
    pub async fn find_leaderboard(&self, app_id: u32, name: &str) -> Result<Leaderboard> {
        self.find_or_create_leaderboard(CMsgClientLbsFindOrCreateLb {
            app_id: Some(app_id),
            leaderboard_name: Some(name.to_string()),
            create_if_not_found: Some(false),
            ..Default::default()
        })
        .await
    }

    /// Finds a leaderboard, creating it if needed.
    pub async fn create_leaderboard(
        &self,
        app_id: u32,
        name: &str,
        sort_method: ELeaderboardSortMethod,
        display_type: ELeaderboardDisplayType,
    ) -> Result<Leaderboard> {
        self.find_or_create_leaderboard(CMsgClientLbsFindOrCreateLb {
            app_id: Some(app_id),
            leaderboard_name: Some(name.to_string()),
            leaderboard_sort_method: Some(sort_method.0),
            leaderboard_display_type: Some(display_type.0),
            create_if_not_found: Some(true),
        })
        .await
    }

    async fn leaderboard_entries(&self, body: CMsgClientLbsGetLbEntries) -> Result<Vec<LeaderboardEntry>> {
        let app_id = body.app_id() as u32;
        let response: CMsgClientLbsGetLbEntriesResponse =
            self.call(routed(EMsg::ClientLBSGetLBEntries, app_id, body)).await?.decode()?;
        check(response.eresult())?;
        Ok(response
            .entries
            .iter()
            .map(|e| LeaderboardEntry {
                global_rank: e.global_rank(),
                score: e.score(),
                steam_id: SteamId::from_u64(e.steam_id_user()),
                ugc_id: UgcHandle::from(e.ugc_id()),
                details: e.details().chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect(),
            })
            .collect())
    }

    /// Requests a range of leaderboard entries.
    pub async fn get_leaderboard_entries(
        &self,
        app_id: u32,
        id: i32,
        range_start: i32,
        range_end: i32,
        data_request: ELeaderboardDataRequest,
    ) -> Result<Vec<LeaderboardEntry>> {
        self.leaderboard_entries(CMsgClientLbsGetLbEntries {
            app_id: Some(app_id as i32),
            leaderboard_id: Some(id),
            leaderboard_data_request: Some(data_request.0),
            range_start: Some(range_start),
            range_end: Some(range_end),
            ..Default::default()
        })
        .await
    }

    /// Requests the leaderboard entries of specific users.
    pub async fn download_leaderboard_entries_for_users(
        &self,
        app_id: u32,
        id: i32,
        users: &[SteamId],
    ) -> Result<Vec<LeaderboardEntry>> {
        self.leaderboard_entries(CMsgClientLbsGetLbEntries {
            app_id: Some(app_id as i32),
            leaderboard_id: Some(id),
            leaderboard_data_request: Some(ELeaderboardDataRequest::Users.0),
            steamids: users.iter().map(|u| u.as_u64()).collect(),
            ..Default::default()
        })
        .await
    }
}
