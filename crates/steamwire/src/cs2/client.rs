//! A session with CS2's game coordinator: inspects items, asks for profiles and matches, keeps the inventory, and
//! passes on what else the game coordinator says.
//!
//! As node-globaloffensive (v3.3.0) does: play CS2, send `ClientHello` until the game coordinator answers
//! `ClientWelcome`, then send each request and wait for its answer, told from others by what it's about.
//!
//! Ported from SteamHelper-rs `steam-client` (`gc/cs2/client.rs`, MIT,
//! Copyright (c) 2020 Martin Mariano).

use std::collections::VecDeque;
use std::mem;
use std::time::Duration;

use crate::enums::EMsg;
use crate::handlers::game_coordinator::{GcMessage, GcSubscription};
use crate::messages::gc::GcPacket;
use crate::protos::csgo::{
    CMsgClientHello, CMsgClientWelcome, CMsgConnectionStatus, CMsgGccStrike15V2Client2GcEconPreviewDataBlockRequest,
    CMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse, CMsgGccStrike15V2ClientLogonFatalError,
    CMsgGccStrike15V2ClientRequestPlayersProfile, CMsgGccStrike15V2MatchList,
    CMsgGccStrike15V2MatchListRequestCurrentLiveGames, CMsgGccStrike15V2MatchListRequestFullGameInfo,
    CMsgGccStrike15V2MatchListRequestLiveGameForUser, CMsgGccStrike15V2MatchListRequestRecentUserGames,
    CMsgGccStrike15V2MatchmakingGc2ClientHello, CMsgGccStrike15V2PlayersProfile, ECsgoGcMsg, EgcBaseClientMsg,
    GcConnectionStatus,
};
use crate::protos::steam::{c_msg_client_games_played::GamePlayed, CMsgClientGamesPlayed};
use crate::{ProtoMsg, SteamClient, SteamId};
use prost::Message;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant};
use tracing::{debug, warn};

use crate::cs2::error::{Error, Result};
use crate::cs2::inventory::{self, Change, KeptInventory, SO_CACHE_SUBSCRIPTION_REFRESH};
use crate::cs2::{
    InspectLink, Inventory, InventoryItem, ItemDetails, ItemDetailsError, MatchList, PlayerProfile, ShareCode,
};

/// CS2's app id.
pub const APP_ID: u32 = 730;
/// How long [`Cs2::connect`] (and a request after the session was lost) waits for the game coordinator's welcome.
pub const GREET_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a request of any kind waits for the game coordinator's answer by default, as node-globaloffensive's
/// `inspectItem` (its other requests wait forever).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The time between two requests of any kind by default, from one sent to the next, as csfloat's inspect service
/// waits (`request_delay`): the game coordinator drops requests that come faster, without a word.
pub const REQUEST_COOLDOWN: Duration = Duration::from_millis(1100);
/// How many events wait for [`Cs2::next_event`]; past this, newer ones are dropped and a [`Cs2Event::Lagged`]
/// counts them.
pub const EVENT_CAPACITY: usize = 256;

/// The first hello waits this long, as node-globaloffensive's: Steam has just been told the account plays CS2.
const FIRST_HELLO: Duration = Duration::from_millis(500);
/// The time until the second hello, doubled after each one, up to [`MAX_HELLO_INTERVAL`].
const HELLO_INTERVAL: Duration = Duration::from_secs(2);
const MAX_HELLO_INTERVAL: Duration = Duration::from_secs(60);
/// The version of the game node-globaloffensive (v3.3.0) says it is, in its hello.
const CLIENT_VERSION: u32 = 2000244;
/// What node-globaloffensive and ValvePython's csgo ask a profile with: what it is, nobody says.
const PROFILE_REQUEST_LEVEL: u32 = 32;

const CLIENT_WELCOME: u32 = EgcBaseClientMsg::KEMsgGcClientWelcome as u32;
const CLIENT_HELLO: u32 = EgcBaseClientMsg::KEMsgGcClientHello as u32;
const CONNECTION_STATUS: u32 = EgcBaseClientMsg::KEMsgGcClientConnectionStatus as u32;
const LOGON_FATAL_ERROR: u32 = ECsgoGcMsg::KEMsgGccStrike15V2ClientLogonFatalError as u32;
const PREVIEW_REQUEST: u32 = ECsgoGcMsg::KEMsgGccStrike15V2Client2GcEconPreviewDataBlockRequest as u32;
const PREVIEW_RESPONSE: u32 = ECsgoGcMsg::KEMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse as u32;
const MATCHMAKING_HELLO: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchmakingGc2ClientHello as u32;
const REQUEST_PLAYERS_PROFILE: u32 = ECsgoGcMsg::KEMsgGccStrike15V2ClientRequestPlayersProfile as u32;
const PLAYERS_PROFILE: u32 = ECsgoGcMsg::KEMsgGccStrike15V2PlayersProfile as u32;
const MATCH_LIST: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchList as u32;
const LIVE_GAMES: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchListRequestCurrentLiveGames as u32;
const RECENT_GAMES: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchListRequestRecentUserGames as u32;
const FULL_GAME_INFO: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchListRequestFullGameInfo as u32;
const LIVE_GAME_FOR_USER: u32 = ECsgoGcMsg::KEMsgGccStrike15V2MatchListRequestLiveGameForUser as u32;

/// A session with CS2's game coordinator, over a logged-on [`SteamClient`]: inspects items, asks for profiles and
/// matches, one request at a time, keeps the account's inventory, and passes on the rest as
/// [events](Self::next_event).
///
/// ```no_run
/// # async fn run(client: steamwire::SteamClient) -> steamwire::cs2::Result<()> {
/// use steamwire::cs2::Cs2;
///
/// let mut cs2 = Cs2::connect(&client).await?;
/// let link = concat!(
///     "steam://rungame/730/76561202255233023/+csgo_econ_action_preview",
///     "%20S76561198084749846A698323590D7935523998312483177",
/// );
/// let item = cs2.inspect_str(link).await?;
/// println!("pattern {:?}, float {:?}", item.pattern(), item.float());
/// if let Some(inventory) = cs2.inventory()? {
///     println!("{} items in the inventory", inventory.len());
/// }
/// # Ok(())
/// # }
/// ```
///
/// The account must own CS2 (the free license). Each request waits [`REQUEST_COOLDOWN`] after the request before it,
/// whatever its kind, and gives up after [`REQUEST_TIMEOUT`]: change them with [`with_cooldown`](Self::with_cooldown)
/// and [`with_timeout`](Self::with_timeout). One request at a time, per account: more accounts ask more.
///
/// When the game coordinator says it has no session for the account, the next request greets it again before
/// asking, as [`next_event`](Self::next_event) does while waiting. When the client disconnects, the session ends for
/// good: requests and events fail with [`Error::Closed`]. Nothing runs in the background: the session's messages are
/// read during requests and by [`next_event`](Self::next_event) and [`try_next_event`](Self::try_next_event); those
/// a request doesn't wait for become [events](Cs2Event), kept until read (up to [`EVENT_CAPACITY`]).
///
/// The session takes CS2's game coordinator messages from the client's [`Events`](crate::Events)
/// ([`SteamClient::subscribe_gc`]) while it lives. The account keeps playing CS2 until the games played change or it
/// logs off.
#[derive(Debug)]
pub struct Cs2 {
    link: Link,
    /// The account, whose inventory is kept.
    account: SteamId,
    session: Session,
    /// Welcomed once: the welcomes after the first are events.
    welcomed_before: bool,
    cooldown: Duration,
    timeout: Duration,
    /// When the latest request was sent, which the cooldown counts from.
    last_request: Option<Instant>,
    events: EventQueue,
    inventory: KeptInventory,
    matchmaking_stats: Option<PlayerProfile>,
}

/// Where the GC messages go and come from: the client, or a test's channels.
#[derive(Debug)]
enum Link {
    Client {
        client: SteamClient,
        gc: GcSubscription,
    },
    #[cfg(test)]
    Fake {
        sent: mpsc::UnboundedSender<GcPacket>,
        gc: mpsc::UnboundedReceiver<GcMessage>,
    },
}

impl Link {
    fn send<M: Message>(&self, msg_type: u32, body: &M) -> Result<()> {
        let packet = GcPacket::proto(msg_type, body);
        match self {
            Self::Client { client, .. } => client.send_gc(APP_ID, &packet)?,
            #[cfg(test)]
            Self::Fake { sent, .. } => sent.send(packet).map_err(|_| Error::Closed)?,
        }
        Ok(())
    }

    /// The next message; `None` once the client disconnected.
    async fn recv(&mut self) -> Option<GcMessage> {
        match self {
            Self::Client { gc, .. } => gc.recv().await,
            #[cfg(test)]
            Self::Fake { gc, .. } => gc.recv().await,
        }
    }

    /// The next message if one waits: `None` if none does, `Some(None)` once the client disconnected.
    fn try_recv(&mut self) -> Option<Option<GcMessage>> {
        let received = match self {
            Self::Client { gc, .. } => gc.try_recv(),
            #[cfg(test)]
            Self::Fake { gc, .. } => gc.try_recv(),
        };
        match received {
            Ok(message) => Some(Some(message)),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(None),
        }
    }
}

/// Where the session with the game coordinator is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Session {
    /// Welcomed, and no word since that the session ended.
    Welcomed,
    /// No session, and no hello sent since it ended.
    Lost,
    /// Greeting: the next hello is due at `next_hello`, and the one after it `interval` later.
    Greeting { next_hello: Instant, interval: Duration },
}

/// What happens in a [`Cs2`] session that no request waits for, from [`Cs2::next_event`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Cs2Event {
    /// The game coordinator says it has no session for the account. The next request greets it again, as
    /// `next_event` does while waiting. Never dropped.
    SessionLost,
    /// The game coordinator welcomed the account again, after the session was lost or unasked (it restarted). The
    /// inventory comes with it, as the item events and [`InventoryLoaded`](Self::InventoryLoaded) that follow. Never
    /// dropped.
    Welcomed,
    /// The account's own matchmaking stats (ranks, wins, level, cooldown…), which the game coordinator sends after
    /// its welcome and when they change. Also kept: [`Cs2::matchmaking_stats`].
    MatchmakingStats(PlayerProfile),
    /// The inventory is current again, or was sent whole again (`CacheSubscribed`, alone or in a welcome): what
    /// changed since the last one known came just before, as item events. Never dropped.
    InventoryLoaded,
    /// The inventory isn't current any more, so [`Cs2::inventory`] gives none: the session ended, a message of the
    /// game coordinator couldn't be read, or it unsubscribed the account's cache or holds another version of it.
    /// Changes that come meanwhile are no events; [`InventoryLoaded`](Self::InventoryLoaded) follows once a copy
    /// comes again, after what changed. Never dropped.
    InventoryUnavailable,
    /// An item came into the inventory, or out of a storage unit.
    ItemAcquired(InventoryItem),
    /// An item changed, such as its position, name or stickers.
    ItemChanged {
        /// As it was.
        old: InventoryItem,
        /// As it is.
        new: InventoryItem,
    },
    /// An item left the inventory, or went into a storage unit.
    ItemRemoved(InventoryItem),
    /// A message `Cs2` doesn't read, or an answer no request waits for (any more).
    Other(GcMessage),
    /// This many events were dropped because they weren't read in time. The inventory stays right: dropped events
    /// are only events.
    Lagged(u64),
}

impl Cs2 {
    /// Plays CS2 on `client`'s account (sends `ClientGamesPlayed` with only CS2), and greets the game coordinator
    /// until it welcomes the account: a hello after half a second, then after 2 s, 4 s… up to a minute apart, as
    /// node-globaloffensive does. The welcome carries the inventory.
    ///
    /// Takes CS2's game coordinator's messages from any earlier subscription of the app
    /// ([`SteamClient::subscribe_gc`]).
    ///
    /// Fails with [`Error::NotLoggedOn`] without a logged-on account, [`Error::Timeout`] if no welcome came within
    /// [`GREET_TIMEOUT`], [`Error::Refused`] if the game coordinator refused the session, and [`Error::Closed`] once
    /// the client disconnected.
    pub async fn connect(client: &SteamClient) -> Result<Self> {
        let account = client.steam_id().filter(|_| client.is_logged_on()).ok_or(Error::NotLoggedOn)?;
        // subscribed first, so that the welcome isn't missed
        let gc = client.subscribe_gc(APP_ID);
        client.send(ProtoMsg::new(
            EMsg::ClientGamesPlayed,
            CMsgClientGamesPlayed {
                games_played: vec![GamePlayed { game_id: Some(u64::from(APP_ID)), ..Default::default() }],
                ..Default::default()
            },
        ))?;
        let mut cs2 = Self::new(Link::Client { client: client.clone(), gc }, account);
        cs2.greet().await?;
        Ok(cs2)
    }

    fn new(link: Link, account: SteamId) -> Self {
        Self {
            link,
            account,
            session: Session::Lost,
            welcomed_before: false,
            cooldown: REQUEST_COOLDOWN,
            timeout: REQUEST_TIMEOUT,
            last_request: None,
            events: EventQueue::default(),
            inventory: KeptInventory::default(),
            matchmaking_stats: None,
        }
    }

    /// Waits `cooldown` between two requests, from one sent to the next, instead of [`REQUEST_COOLDOWN`].
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// Waits `timeout` for each answer, instead of [`REQUEST_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Whether the game coordinator welcomed the account, and said nothing since that the session ended, as far as
    /// the messages read tell.
    pub fn has_session(&self) -> bool {
        self.session == Session::Welcomed
    }

    /// The account's inventory, if it is the game coordinator's current one: first reads what came since (the
    /// messages become [events](Cs2Event), as during requests), then gives the inventory as of the latest message from
    /// the game coordinator. `None` while it isn't known to be current: before the game coordinator sent it, and
    /// from [`Cs2Event::InventoryUnavailable`] until [`Cs2Event::InventoryLoaded`].
    ///
    /// Fails with [`Error::Closed`] once the client disconnected, and with [`Error::Refused`] if the game
    /// coordinator refused the session.
    pub fn inventory(&mut self) -> Result<Option<&Inventory>> {
        self.read_waiting()?;
        Ok(self.inventory.current())
    }

    /// The account's own matchmaking stats the game coordinator last sent, if it did.
    pub fn matchmaking_stats(&self) -> Option<&PlayerProfile> {
        self.matchmaking_stats.as_ref()
    }

    /// Parses `link` as an [`InspectLink`] and inspects it.
    pub async fn inspect_str(&mut self, link: &str) -> Result<ItemDetails> {
        self.inspect(&link.parse()?).await
    }

    /// The details of the item `link` names, from the game coordinator, or from the link when it embeds them (then
    /// offline, at once).
    ///
    /// First waits for the cooldown, and greets the game coordinator again if the session ended. Then asks
    /// (`Client2GCEconPreviewDataBlockRequest`, 9156), and waits for the answer (9157) about the same asset id:
    /// answers about other items, such as a late one to an earlier request, are passed on as events. Dropping the
    /// future forgets the request; its answer, if it comes, is an event then.
    ///
    /// Fails with [`Error::Timeout`] if no answer came within the timeout (the game coordinator doesn't answer about
    /// items it can't find, nor requests that come too fast), [`Error::ItemDetails`] if the answer has no item or one
    /// without a def index, and as [`connect`](Self::connect) when greeting.
    pub async fn inspect(&mut self, link: &InspectLink) -> Result<ItemDetails> {
        let params = match link.params() {
            Ok(params) => params,
            Err(item) => return Ok(item.details()?),
        };
        let request = CMsgGccStrike15V2Client2GcEconPreviewDataBlockRequest {
            param_s: Some(params.s),
            param_a: Some(params.a),
            param_d: Some(params.d),
            param_m: Some(params.m),
        };
        self.request(PREVIEW_REQUEST, &request, PREVIEW_RESPONSE, |message| {
            let response: CMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse = parse(message)?;
            let Some(item) = response.iteminfo else {
                return Err(ItemDetailsError::NoItem.into());
            };
            if item.itemid() != params.a {
                return Ok(None);
            }
            Ok(Some(ItemDetails::try_from(item)?))
        })
        .await
    }

    /// Asks the game coordinator for the account's matchmaking stats again, as the account's own profile
    /// ([`player_profile`](Self::player_profile)), and keeps them: [`matchmaking_stats`](Self::matchmaking_stats).
    ///
    /// CS2's game coordinator no longer answers `MatchmakingClient2GCHello` (9109), which ValvePython's csgo
    /// `request_matchmaking_stats` sends, nor sends `MatchmakingGC2ClientHello` (9110) after its welcome (both checked
    /// live in November 2026), but it answers the profile request about the account itself.
    ///
    /// Fails as [`inspect`](Self::inspect) does.
    pub async fn request_matchmaking_stats(&mut self) -> Result<PlayerProfile> {
        let stats = self.player_profile(self.account).await?;
        self.matchmaking_stats = Some(stats.clone());
        Ok(stats)
    }

    /// The CS2 profile of `player` (`ClientRequestPlayersProfile`, 9127): ranks, wins, level, commendations. Waits for
    /// the answer (`PlayersProfile`, 9128) whose first profile is `player`'s, as node-globaloffensive matches it. The
    /// game coordinator only answers about players who play CS2 and are friends of the account, node says.
    ///
    /// Fails with [`Error::NotIndividual`] if `player` isn't an individual account, and as
    /// [`inspect`](Self::inspect) does.
    pub async fn player_profile(&mut self, player: SteamId) -> Result<PlayerProfile> {
        let account_id = individual(player)?;
        let request = CMsgGccStrike15V2ClientRequestPlayersProfile {
            account_id: Some(account_id),
            request_level: Some(PROFILE_REQUEST_LEVEL),
            ..Default::default()
        };
        self.request(REQUEST_PLAYERS_PROFILE, &request, PLAYERS_PROFILE, |message| {
            let mut profiles: CMsgGccStrike15V2PlayersProfile = parse(message)?;
            match profiles.account_profiles.first() {
                Some(profile) if profile.account_id() == account_id => {
                    Ok(Some(PlayerProfile::new(profiles.account_profiles.swap_remove(0))))
                }
                _ => Ok(None),
            }
        })
        .await
    }

    /// The details of the match `code` names (`MatchListRequestFullGameInfo`, 9147): its rounds, score, demo… Waits
    /// for the match list (9139) that answers it: the request's type in `msgrequestid`, as ValvePython's csgo tells
    /// them, and the match, unless the game coordinator found none.
    ///
    /// Fails as [`inspect`](Self::inspect) does.
    pub async fn match_info(&mut self, code: &ShareCode) -> Result<MatchList> {
        let request = CMsgGccStrike15V2MatchListRequestFullGameInfo {
            matchid: Some(code.match_id()),
            outcomeid: Some(code.outcome_id()),
            token: Some(u32::from(code.token())),
        };
        let match_id = code.match_id();
        self.match_list(FULL_GAME_INFO, &request, |list| {
            list.matches.is_empty() || list.matches.iter().any(|info| info.matchid() == match_id)
        })
        .await
    }

    /// The recent matches of `player` (`MatchListRequestRecentUserGames`, 9141), as the game's "Your Matches" lists
    /// them. Waits for the match list (9139) that answers it, as [`match_info`](Self::match_info) does.
    ///
    /// Fails as [`player_profile`](Self::player_profile) does.
    pub async fn recent_matches(&mut self, player: SteamId) -> Result<MatchList> {
        let account_id = individual(player)?;
        let request = CMsgGccStrike15V2MatchListRequestRecentUserGames { accountid: Some(account_id) };
        self.match_list(RECENT_GAMES, &request, |list| list.accountid.is_none_or(|id| id == account_id)).await
    }

    /// The match `player` is playing (`MatchListRequestLiveGameForUser`, 9154). ValvePython's csgo says the game
    /// coordinator doesn't answer it any more: then this fails with [`Error::Timeout`].
    ///
    /// Fails as [`player_profile`](Self::player_profile) does.
    pub async fn live_match(&mut self, player: SteamId) -> Result<MatchList> {
        let account_id = individual(player)?;
        let request = CMsgGccStrike15V2MatchListRequestLiveGameForUser { accountid: Some(account_id) };
        self.match_list(LIVE_GAME_FOR_USER, &request, |list| list.accountid.is_none_or(|id| id == account_id)).await
    }

    /// The tournament matches being played (`MatchListRequestCurrentLiveGames`, 9140), as the game's "Watch" lists
    /// them.
    ///
    /// Fails as [`inspect`](Self::inspect) does.
    pub async fn live_matches(&mut self) -> Result<MatchList> {
        let request = CMsgGccStrike15V2MatchListRequestCurrentLiveGames::default();
        self.match_list(LIVE_GAMES, &request, |_| true).await
    }

    /// The next [event](Cs2Event): one kept, or the next one the game coordinator causes. While the session is lost,
    /// greets the game coordinator as [`connect`](Self::connect) does, without giving up, until it welcomes the
    /// account ([`Cs2Event::Welcomed`]). Cancel-safe: it can wait in a `select!`.
    ///
    /// Fails with [`Error::Closed`] once the client disconnected and every event was read, and with
    /// [`Error::Refused`] if the game coordinator refused the session (the next call greets it again).
    ///
    /// ```no_run
    /// # async fn run(mut cs2: steamwire::cs2::Cs2) -> steamwire::cs2::Result<()> {
    /// use steamwire::cs2::Cs2Event;
    ///
    /// loop {
    ///     match cs2.next_event().await? {
    ///         Cs2Event::ItemAcquired(item) => println!("Acquired {}", item.id()),
    ///         Cs2Event::ItemChanged { new, .. } => println!("Changed {}", new.id()),
    ///         Cs2Event::ItemRemoved(item) => println!("Removed {}", item.id()),
    ///         Cs2Event::MatchmakingStats(stats) => println!("Level {:?}", stats.level()),
    ///         Cs2Event::SessionLost => println!("Lost the session, greeting again"),
    ///         Cs2Event::Welcomed => println!("Welcomed again"),
    ///         Cs2Event::InventoryLoaded => {
    ///             let items = cs2.inventory()?.map_or(0, |inventory| inventory.len());
    ///             println!("The inventory is current: {} items", items);
    ///         }
    ///         Cs2Event::Lagged(missed) => println!("Missed {} events", missed),
    ///         _ => {}
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn next_event(&mut self) -> Result<Cs2Event> {
        loop {
            if let Some(event) = self.events.pop() {
                return Ok(event);
            }
            match self.hello_due() {
                Some(next_hello) => tokio::select! {
                    message = self.link.recv() => self.handle_for_events(message)?,
                    () = sleep_until(next_hello) => self.send_hello()?,
                },
                None => {
                    let message = self.link.recv().await;
                    self.handle_for_events(message)?;
                }
            }
        }
    }

    /// The next [event](Cs2Event) if one waits, without waiting nor greeting.
    ///
    /// Fails as [`next_event`](Self::next_event) does.
    pub fn try_next_event(&mut self) -> Result<Option<Cs2Event>> {
        loop {
            if let Some(event) = self.events.pop() {
                return Ok(Some(event));
            }
            match self.link.try_recv() {
                Some(message) => self.handle_for_events(message)?,
                None => return Ok(None),
            }
        }
    }

    /// Asks for a match list with `body`, a request of type `msg_type`, and waits for the one about it.
    async fn match_list<M: Message>(
        &mut self,
        msg_type: u32,
        body: &M,
        about: impl Fn(&CMsgGccStrike15V2MatchList) -> bool,
    ) -> Result<MatchList> {
        self.request(msg_type, body, MATCH_LIST, |message| {
            let list: CMsgGccStrike15V2MatchList = parse(message)?;
            Ok((list.msgrequestid() == msg_type && about(&list)).then(|| MatchList::new(list)))
        })
        .await
    }

    /// Sends `body`, a request of type `msg_type`, once the cooldown passed and the game coordinator welcomed the
    /// account, then waits for its answer: the first message of type `answer_type` that `answer` reads as one. The
    /// others are kept as events.
    async fn request<M: Message, T>(
        &mut self,
        msg_type: u32,
        body: &M,
        answer_type: u32,
        mut answer: impl FnMut(&GcMessage) -> Result<Option<T>>,
    ) -> Result<T> {
        if let Some(last) = self.last_request {
            sleep_until(last + self.cooldown).await;
        }
        self.read_waiting()?;
        if !self.has_session() {
            self.greet().await?;
        }

        self.link.send(msg_type, body)?;
        let sent = Instant::now();
        self.last_request = Some(sent);

        let deadline = sent + self.timeout;
        loop {
            let message = tokio::select! {
                message = self.link.recv() => message,
                () = sleep_until(deadline) => return Err(Error::Timeout(answer_type)),
            };
            if let Some(message) = &message {
                if message.msg_type() == answer_type {
                    if let Some(answer) = answer(message)? {
                        return Ok(answer);
                    }
                    debug!("passing on an answer of CS2's game coordinator about something else");
                }
            }
            self.handle(message)?;
        }
    }

    /// Sends hellos until the game coordinator welcomes the account, for up to [`GREET_TIMEOUT`].
    async fn greet(&mut self) -> Result<()> {
        let deadline = Instant::now() + GREET_TIMEOUT;
        while let Some(next_hello) = self.hello_due() {
            tokio::select! {
                message = self.link.recv() => self.handle(message)?,
                () = sleep_until(next_hello) => self.send_hello()?,
                () = sleep_until(deadline) => return Err(Error::Timeout(CLIENT_WELCOME)),
            }
        }
        Ok(())
    }

    /// When the next hello is due, unless welcomed. Starts greeting if the session was lost.
    fn hello_due(&mut self) -> Option<Instant> {
        match self.session {
            Session::Welcomed => None,
            Session::Lost => {
                let next_hello = Instant::now() + FIRST_HELLO;
                self.session = Session::Greeting { next_hello, interval: HELLO_INTERVAL };
                Some(next_hello)
            }
            Session::Greeting { next_hello, .. } => Some(next_hello),
        }
    }

    fn send_hello(&mut self) -> Result<()> {
        let Session::Greeting { interval, .. } = self.session else {
            return Ok(());
        };
        debug!("greeting CS2's game coordinator, again in {interval:?}");
        self.link.send(CLIENT_HELLO, &hello())?;
        self.session = Session::Greeting {
            next_hello: Instant::now() + interval,
            interval: (interval * 2).min(MAX_HELLO_INTERVAL),
        };
        Ok(())
    }

    /// Reads what came since the last request, to know whether the session is still on.
    fn read_waiting(&mut self) -> Result<()> {
        while let Some(message) = self.link.try_recv() {
            self.handle(message)?;
        }
        Ok(())
    }

    /// As [`handle`](Self::handle), but the end of the session the disconnection brings is read as events first.
    fn handle_for_events(&mut self, message: Option<GcMessage>) -> Result<()> {
        match self.handle(message) {
            Err(Error::Closed) if !self.events.is_empty() => Ok(()),
            result => result,
        }
    }

    /// Follows the session, the inventory and the stats through `message`, and keeps what's for the caller as
    /// events. `None`: the client disconnected.
    fn handle(&mut self, message: Option<GcMessage>) -> Result<()> {
        let Some(message) = message else {
            self.lose_session();
            return Err(Error::Closed);
        };
        match message.msg_type() {
            CLIENT_WELCOME => self.welcomed(&message)?,
            CONNECTION_STATUS => match parse::<CMsgConnectionStatus>(&message) {
                // the raw value, as node-globaloffensive compares it: one unknown here isn't a session
                Ok(status) => {
                    let status = status.status.unwrap_or(0);
                    if status != GcConnectionStatus::HaveSession as i32 {
                        debug!("CS2's game coordinator has no session for the account (status {status})");
                        self.lose_session();
                    }
                }
                Err(error) => warn!("unreadable connection status from CS2's game coordinator: {error}"),
            },
            LOGON_FATAL_ERROR => {
                self.lose_session();
                self.session = Session::Lost;
                let error: CMsgGccStrike15V2ClientLogonFatalError = parse(&message)?;
                return Err(Error::Refused { code: error.errorcode(), message: error.message().to_string() });
            }
            MATCHMAKING_HELLO => match parse::<CMsgGccStrike15V2MatchmakingGc2ClientHello>(&message) {
                Ok(stats) => {
                    let stats = PlayerProfile::new(stats);
                    self.matchmaking_stats = Some(stats.clone());
                    self.events.push(Cs2Event::MatchmakingStats(stats));
                }
                Err(error) => {
                    warn!("unreadable matchmaking stats from CS2's game coordinator: {error}");
                    self.events.push(Cs2Event::Other(message));
                }
            },
            _ => match self.inventory.apply(&message, self.account, self.has_session()) {
                Some(changes) => self.changed(changes)?,
                None => self.events.push(Cs2Event::Other(message)),
            },
        }
        Ok(())
    }

    /// Passes on how the inventory changed, as events, and asks for it again if it needs.
    fn changed(&mut self, changes: Vec<Change>) -> Result<()> {
        for change in changes {
            self.events.push(match change {
                Change::Refresh => {
                    self.ask_for_inventory()?;
                    continue;
                }
                Change::Loaded => Cs2Event::InventoryLoaded,
                Change::Unavailable => Cs2Event::InventoryUnavailable,
                Change::Acquired(item) => Cs2Event::ItemAcquired(item),
                Change::Changed { old, new } => Cs2Event::ItemChanged { old, new },
                Change::Removed(item) => Cs2Event::ItemRemoved(item),
            });
        }
        Ok(())
    }

    /// Asks the game coordinator to send the account's cache again (`CacheSubscriptionRefresh`), as the game does when
    /// its copy may be old.
    fn ask_for_inventory(&mut self) -> Result<()> {
        debug!("asking CS2's game coordinator for the inventory again");
        self.link.send(SO_CACHE_SUBSCRIPTION_REFRESH, &inventory::refresh(self.account))
    }

    /// Reads the welcome: the session is on, with the inventory it carries. The first welcome is no event: `connect`
    /// returns with it.
    fn welcomed(&mut self, message: &GcMessage) -> Result<()> {
        debug!("CS2's game coordinator welcomed the account");
        self.session = Session::Welcomed;
        let first = !mem::replace(&mut self.welcomed_before, true);
        if !first {
            self.events.push(Cs2Event::Welcomed);
        }
        let mut changes = match parse::<CMsgClientWelcome>(message) {
            Ok(welcome) => self.inventory.welcome(&welcome, self.account),
            Err(error) => {
                warn!("unreadable welcome from CS2's game coordinator: {error}");
                // a new session, without the cache it brings
                let mut changes = vec![];
                if self.inventory.invalidate() {
                    changes.push(Change::Unavailable);
                }
                changes
            }
        };
        if first {
            changes.retain(|change| !matches!(change, Change::Loaded));
        }
        self.changed(changes)
    }

    /// The session ended, if it was on, and with it what's known of the inventory.
    fn lose_session(&mut self) {
        if self.session == Session::Welcomed {
            self.session = Session::Lost;
            self.events.push(Cs2Event::SessionLost);
        }
        if self.inventory.invalidate() {
            self.events.push(Cs2Event::InventoryUnavailable);
        }
    }
}

/// Decodes a message's body.
fn parse<M: Message + Default>(message: &GcMessage) -> Result<M> {
    Ok(M::decode(message.packet.body.clone())?)
}

/// The hello node-globaloffensive (v3.3.0) sends.
fn hello() -> CMsgClientHello {
    CMsgClientHello {
        version: Some(CLIENT_VERSION),
        client_session_need: Some(0),
        client_launcher: Some(0),
        steam_launcher: Some(0),
        ..Default::default()
    }
}

/// The account id of `player`, if an individual account in the public universe, as node-globaloffensive checks.
fn individual(player: SteamId) -> Result<u32> {
    let account_id = player.account_id();
    if account_id == 0 || SteamId::from_account_id(account_id) != player {
        return Err(Error::NotIndividual(player));
    }
    Ok(account_id)
}

/// The events [`Cs2::next_event`] hasn't read.
#[derive(Debug, Default)]
struct EventQueue {
    events: VecDeque<Cs2Event>,
    /// Events dropped since the last `Lagged`, which is added when there is room again.
    dropped: u64,
}

impl EventQueue {
    /// Keeps `event`, or drops it past [`EVENT_CAPACITY`], unless it's about the session or whether the inventory is
    /// current. Those are never dropped, and one is enough while nothing came after it.
    fn push(&mut self, event: Cs2Event) {
        let about_session = matches!(
            event,
            Cs2Event::SessionLost | Cs2Event::Welcomed | Cs2Event::InventoryLoaded | Cs2Event::InventoryUnavailable
        );
        if !about_session && self.events.len() >= EVENT_CAPACITY {
            if self.dropped == 0 {
                debug!("CS2's events aren't read: dropping them");
            }
            self.dropped += 1;
            return;
        }
        self.flush_dropped();
        if about_session {
            let last = self.events.iter().rev().find(|event| !matches!(event, Cs2Event::Lagged(_)));
            if last.map(mem::discriminant) == Some(mem::discriminant(&event)) {
                return;
            }
        }
        self.events.push_back(event);
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty() && self.dropped == 0
    }

    fn pop(&mut self) -> Option<Cs2Event> {
        let Some(event) = self.events.pop_front() else {
            // the events dropped came after every event read
            return (self.dropped > 0).then(|| Cs2Event::Lagged(mem::take(&mut self.dropped)));
        };
        // a burst of events doesn't keep its memory
        if self.events.is_empty() && self.events.capacity() > EVENT_CAPACITY {
            self.events.shrink_to_fit();
        }
        Some(event)
    }

    fn flush_dropped(&mut self) {
        if self.dropped > 0 {
            self.events.push_back(Cs2Event::Lagged(mem::take(&mut self.dropped)));
        }
    }
}

#[cfg(test)]
mod tests;
