//! Matches, as the game coordinator lists them: a match's details from its share code, a player's recent matches,
//! the live ones.
//!
//! Ported from SteamHelper-rs `steam-client` (`gc/cs2/matches.rs`, MIT,
//! Copyright (c) 2020 Martin Mariano).

use std::fmt;
use std::slice;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use steamwire::SteamId;
use steamwire_protos::csgo::{
    CDataGccStrike15V2MatchInfo, CMsgGccStrike15V2MatchList, CMsgGccStrike15V2MatchmakingServerRoundStats,
};

use crate::ShareCode;

/// The matches the game coordinator gives (`MatchList`, 9139), in answer to a request about matches.
///
/// What the methods don't give, such as the tournament, is in [`raw`](Self::raw).
///
/// ```no_run
/// # async fn run(mut cs2: steamwire_cs2::Cs2) -> Result<(), Box<dyn std::error::Error>> {
/// use steamwire_cs2::ShareCode;
///
/// let code: ShareCode = "CSGO-vGqmL-vb2x2-nTQtU-e9dco-qWHzN".parse()?;
/// // no matches when the game coordinator found none
/// for info in cs2.match_info(&code).await?.matches() {
///     println!("{:?}: {:?}", info.map(), info.score());
///     println!("demo {:?}", info.demo_url());
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone, PartialEq)]
pub struct MatchList {
    proto: CMsgGccStrike15V2MatchList,
}

impl MatchList {
    pub(crate) fn new(proto: CMsgGccStrike15V2MatchList) -> Self {
        Self { proto }
    }

    /// The matches. None when the game coordinator found none.
    pub fn matches(&self) -> impl ExactSizeIterator<Item = MatchInfo<'_>> {
        self.proto.matches.iter().map(|proto| MatchInfo { proto })
    }

    /// The player the list is about, for a player's recent or live matches.
    pub fn player(&self) -> Option<SteamId> {
        self.proto.accountid.filter(|&id| id != 0).map(SteamId::from_account_id)
    }

    /// The message the game coordinator gave.
    pub fn raw(&self) -> &CMsgGccStrike15V2MatchList {
        &self.proto
    }

    /// The message the game coordinator gave.
    pub fn into_proto(self) -> CMsgGccStrike15V2MatchList {
        self.proto
    }
}

/// Debug shows what the methods give.
impl fmt::Debug for MatchList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatchList")
            .field("player", &self.player())
            .field("matches", &self.matches().collect::<Vec<_>>())
            .finish()
    }
}

/// A match of a [`MatchList`]: a `CDataGCCStrike15_v2_MatchInfo`. Its rounds' stats are cumulative: the last round
/// has the match's totals.
///
/// A friend's recent matches, and the share code of each, which [`Cs2::match_info`](crate::Cs2::match_info)
/// takes:
///
/// ```no_run
/// # async fn run(mut cs2: steamwire_cs2::Cs2) -> steamwire_cs2::Result<()> {
/// use steamwire::SteamId;
///
/// let friend = SteamId::from_u64(76561198084749846);
/// for info in cs2.recent_matches(friend).await?.matches() {
///     let code = info.share_code().map(|code| code.to_string());
///     println!("{:?} on {:?}: {:?}", info.time(), info.map(), code);
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, PartialEq)]
pub struct MatchInfo<'a> {
    proto: &'a CDataGccStrike15V2MatchInfo,
}

impl<'a> MatchInfo<'a> {
    /// The match's id. `None` when the game coordinator doesn't say.
    pub fn match_id(&self) -> Option<u64> {
        self.proto.matchid.filter(|&id| id != 0)
    }

    /// When the match was played.
    pub fn time(&self) -> Option<SystemTime> {
        self.proto.matchtime.filter(|&time| time != 0).map(|time| UNIX_EPOCH + Duration::from_secs(u64::from(time)))
    }

    /// The map, such as `de_mirage`, when the game coordinator says.
    pub fn map(&self) -> Option<&'a str> {
        self.proto
            .watchablematchinfo
            .as_ref()
            .and_then(|watchable| watchable.game_map.as_deref())
            .filter(|map| !map.is_empty())
    }

    /// The stats after each round the game coordinator gives, the last one with the match's totals.
    pub fn rounds(&self) -> &'a [CMsgGccStrike15V2MatchmakingServerRoundStats] {
        match self.proto.roundstats_legacy.as_ref() {
            Some(legacy) if self.proto.roundstatsall.is_empty() => slice::from_ref(legacy),
            _ => &self.proto.roundstatsall,
        }
    }

    /// The stats at the end of the match (or the latest round, for a live one).
    pub fn last_round(&self) -> Option<&'a CMsgGccStrike15V2MatchmakingServerRoundStats> {
        self.rounds().last()
    }

    /// Where the match's demo can be downloaded (`.dem.bz2`), while Valve keeps it: the last round's `map`, which
    /// holds a URL for a finished match.
    pub fn demo_url(&self) -> Option<&'a str> {
        self.last_round()
            .and_then(|round| round.map.as_deref())
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
    }

    /// The teams' scores, as the last round has them.
    pub fn score(&self) -> Option<[i32; 2]> {
        match self.last_round()?.team_scores.as_slice() {
            &[first, second] => Some([first, second]),
            _ => None,
        }
    }

    /// How long the match lasted (so far, for a live one).
    pub fn duration(&self) -> Option<Duration> {
        let seconds = self.last_round()?.match_duration?;
        u64::try_from(seconds).ok().map(Duration::from_secs)
    }

    /// The players, in the order of the rounds' per-player stats (`kills`, `deaths`…).
    pub fn players(&self) -> impl ExactSizeIterator<Item = SteamId> + 'a {
        self.last_round()
            .and_then(|round| round.reservation.as_ref())
            .map_or(&[][..], |reservation| reservation.account_ids.as_slice())
            .iter()
            .map(|&id| SteamId::from_account_id(id))
    }

    /// The match's share code, made from its id, its last round's reservation id and its GOTV port, as akiver's
    /// csgo-sharecode `encodeMatch` does.
    pub fn share_code(&self) -> Option<ShareCode> {
        let match_id = self.match_id()?;
        let outcome_id = self.last_round()?.reservationid?;
        let port = self.proto.watchablematchinfo.as_ref()?.tv_port?;
        Some(ShareCode::new(match_id, outcome_id, u16::try_from(port).ok()?))
    }

    /// The message the game coordinator gave.
    pub fn raw(&self) -> &'a CDataGccStrike15V2MatchInfo {
        self.proto
    }
}

/// Debug shows what the methods give.
impl fmt::Debug for MatchInfo<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatchInfo")
            .field("match_id", &self.match_id())
            .field("time", &self.time())
            .field("map", &self.map())
            .field("rounds", &self.rounds().len())
            .field("score", &self.score())
            .field("duration", &self.duration())
            .field("demo_url", &self.demo_url())
            .field("share_code", &self.share_code())
            .finish()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use steamwire_protos::csgo::{CMsgGccStrike15V2MatchmakingGc2ServerReserve, WatchableMatchInfo};

    use super::*;

    const DEMO: &str = "http://replay129.valve.net/730/003400360672356205056_0123456789.dem.bz2";

    /// A finished match: the one of akiver's second share code sample, 16 to 12 on Mirage.
    pub(crate) fn finished(match_id: u64) -> CDataGccStrike15V2MatchInfo {
        let first = CMsgGccStrike15V2MatchmakingServerRoundStats {
            reservationid: Some(3400367402569957763),
            team_scores: vec![1, 0],
            ..Default::default()
        };
        let last = CMsgGccStrike15V2MatchmakingServerRoundStats {
            reservationid: Some(3400367402569957763),
            map: Some(DEMO.to_string()),
            team_scores: vec![16, 12],
            match_duration: Some(2400),
            reservation: Some(CMsgGccStrike15V2MatchmakingGc2ServerReserve {
                account_ids: vec![124484118, 1],
                ..Default::default()
            }),
            ..Default::default()
        };
        CDataGccStrike15V2MatchInfo {
            matchid: Some(match_id),
            matchtime: Some(1_700_000_000),
            watchablematchinfo: Some(WatchableMatchInfo {
                tv_port: Some(9725),
                game_map: Some("de_mirage".to_string()),
                ..Default::default()
            }),
            roundstatsall: vec![first, last],
            ..Default::default()
        }
    }

    #[test]
    fn reads_a_finished_match() {
        let proto = CMsgGccStrike15V2MatchList {
            accountid: Some(124484118),
            matches: vec![finished(3400360672356205056)],
            ..Default::default()
        };
        let list = MatchList::new(proto);
        assert_eq!(list.player(), Some(SteamId::from_u64(76561198084749846)));
        assert_eq!(list.matches().len(), 1);
        let info = list.matches().next().unwrap();
        assert_eq!(info.match_id(), Some(3400360672356205056));
        assert_eq!(info.time(), Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)));
        assert_eq!(info.map(), Some("de_mirage"));
        assert_eq!(info.rounds().len(), 2);
        assert_eq!(info.score(), Some([16, 12]));
        assert_eq!(info.duration(), Some(Duration::from_secs(2400)));
        assert_eq!(info.demo_url(), Some(DEMO));
        let players: Vec<_> = info.players().map(SteamId::account_id).collect();
        assert_eq!(players, [124484118, 1]);
        assert_eq!(info.share_code().unwrap().to_string(), "CSGO-L9spZ-ihuov-cyhtE-kxbqa-FkBAA");
        assert!(format!("{list:?}").contains("de_mirage"));
    }

    #[test]
    fn reads_a_match_with_little() {
        let mut info = CDataGccStrike15V2MatchInfo::default();
        let empty = MatchInfo { proto: &info };
        assert_eq!(empty.match_id(), None);
        assert_eq!(empty.time(), None);
        assert_eq!(empty.map(), None);
        assert!(empty.rounds().is_empty());
        assert_eq!(empty.score(), None);
        assert_eq!(empty.duration(), None);
        assert_eq!(empty.demo_url(), None);
        assert_eq!(empty.players().len(), 0);
        assert_eq!(empty.share_code(), None);

        // a live match: the legacy round stats, a map name where finished ones have the demo
        info.roundstats_legacy = Some(CMsgGccStrike15V2MatchmakingServerRoundStats {
            map: Some("de_inferno".to_string()),
            team_scores: vec![3],
            ..Default::default()
        });
        let live = MatchInfo { proto: &info };
        assert_eq!(live.rounds().len(), 1);
        assert_eq!(live.demo_url(), None);
        assert_eq!(live.score(), None);
    }
}
