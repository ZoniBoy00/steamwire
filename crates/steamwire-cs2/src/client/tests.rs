//! `Cs2` against a fake game coordinator: the session's channels, without a client. SteamHelper-rs's tests, less
//! those about reconnecting and dropped messages, which steamwire has neither of.

use std::future::Future;

use bytes::Bytes;
use steamwire_protos::csgo::c_econ_item_preview_data_block::Sticker as ProtoSticker;
use steamwire_protos::csgo::c_msg_so_multiple_objects::SingleObject;
use steamwire_protos::csgo::{
    CDataGccStrike15V2MatchInfo, CEconItemPreviewDataBlock, CMsgSoCacheSubscribed, CMsgSoCacheSubscriptionCheck,
    CMsgSoCacheSubscriptionRefresh, CMsgSoCacheUnsubscribed, CMsgSoMultipleObjects, CMsgSoSingleObject, CMsgSoidOwner,
    CsoEconItem,
};
use tokio::time::timeout;

use super::*;
use crate::inventory::tests as items;
use crate::inventory::{
    ITEM_TYPE, SO_CACHE_SUBSCRIBED, SO_CACHE_SUBSCRIPTION_CHECK, SO_CACHE_UNSUBSCRIBED, SO_CREATE, SO_DESTROY,
    SO_UPDATE, SO_UPDATE_MULTIPLE,
};
use crate::matches::tests::finished;
use crate::InspectLinkError;

const OWNER: u64 = 76561198084749846;
const ASSET: u64 = 698323590;
const D: u64 = 7935523998312483177;
/// Paintwear 0.20482855, as f32 bits.
const WEAR: u32 = 1045544595;
/// A generated link that embeds an M4A1-S (def index 60), as `item.rs` has it.
const EMBEDDED: &str = "00183C20B803280538E9A3C5DD0340E102C246A0D1";
/// A player, as the inspect links have it.
const PLAYER: u64 = OWNER;
const PLAYER_ACCOUNT: u32 = 124484118;
/// akiver's first share code sample.
const SHARE_CODE: &str = "CSGO-L9spZ-ihuov-cyhtE-kxbqa-FkBAA";
const SHARED_MATCH: u64 = 3400360672356205056;

/// Fails the test instead of hanging it. Time is paused: an hour passes at once when nothing else can happen.
async fn soon<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(3600), future).await.expect("hung")
}

/// The game coordinator's end of a session: what `Cs2` sends, and a way to answer.
struct Gc {
    sent: mpsc::UnboundedReceiver<GcPacket>,
    /// `None` once the client "disconnected".
    to_cs2: Option<mpsc::UnboundedSender<GcMessage>>,
    account: SteamId,
}

impl Gc {
    /// The next message to the game coordinator: its type and body.
    async fn sent_to_gc(&mut self) -> (u32, Bytes) {
        let packet = self.sent.recv().await.expect("`Cs2` was dropped");
        assert!(packet.is_proto());
        (packet.msg_type, packet.body)
    }

    /// The next message to the game coordinator if one was sent.
    fn try_to_gc(&mut self) -> Option<(u32, Bytes)> {
        self.sent.try_recv().ok().map(|packet| (packet.msg_type, packet.body))
    }

    /// Reads a hello, as node-globaloffensive sends it. Returns when it came.
    async fn hello_read(&mut self) -> Instant {
        let (msg_type, body) = self.sent_to_gc().await;
        assert_eq!(msg_type, CLIENT_HELLO);
        let hello = CMsgClientHello::decode(body).unwrap();
        assert_eq!(hello.version(), 2000244);
        assert_eq!(hello, super::hello());
        Instant::now()
    }

    /// Reads the next message to the game coordinator, which must be of type `msg_type`, as `M`. Returns it and when
    /// it came.
    async fn read_request<M: Message + Default>(&mut self, msg_type: u32) -> (M, Instant) {
        let (got, body) = self.sent_to_gc().await;
        assert_eq!(got, msg_type);
        (M::decode(body).unwrap(), Instant::now())
    }

    /// Reads an inspect request. Returns it and when it came.
    async fn request_read(&mut self) -> (CMsgGccStrike15V2Client2GcEconPreviewDataBlockRequest, Instant) {
        self.read_request(PREVIEW_REQUEST).await
    }

    /// Reads a request for the account's cache.
    fn assert_refresh_sent(&mut self) {
        let (msg_type, body) = self.try_to_gc().expect("a request for the cache");
        assert_eq!(msg_type, SO_CACHE_SUBSCRIPTION_REFRESH);
        let refresh = CMsgSoCacheSubscriptionRefresh::decode(body).unwrap();
        assert_eq!(refresh.owner_soid, Some(owner_soid(self.account)));
    }

    /// Sends `body`, of type `msg_type`, from CS2's game coordinator.
    fn send<M: Message>(&self, msg_type: u32, body: &M) {
        self.push(gc_message(msg_type, body));
    }

    fn push(&self, message: GcMessage) {
        self.to_cs2.as_ref().expect("disconnected").send(message).unwrap();
    }

    fn welcome(&self) {
        self.send(CLIENT_WELCOME, &CMsgClientWelcome::default());
    }

    /// Checks `Cs2` sends nothing more to the game coordinator for `wait`.
    async fn quiet(&mut self, wait: Duration) {
        if let Ok(Some(packet)) = timeout(wait, self.sent.recv()).await {
            panic!("sent the game coordinator a {}", packet.msg_type);
        }
    }

    /// The client disconnects.
    fn disconnect(&mut self) {
        self.to_cs2 = None;
    }
}

/// A `Cs2` for the account, on a fake game coordinator, not welcomed yet.
fn fake() -> (Cs2, Gc) {
    let (sent_tx, sent) = mpsc::unbounded_channel();
    let (to_cs2, gc) = mpsc::unbounded_channel();
    let account = SteamId::from_u64(OWNER);
    let cs2 = Cs2::new(Link::Fake { sent: sent_tx, gc }, account);
    (cs2, Gc { sent, to_cs2: Some(to_cs2), account })
}

/// A session the game coordinator welcomed with the account's cache, without items.
async fn session() -> (Cs2, Gc) {
    session_welcomed(|account| welcome_with(account, &[])).await
}

/// A session the game coordinator welcomed with `welcome(account)`, as `connect` greets it.
async fn session_welcomed(welcome: impl FnOnce(SteamId) -> CMsgClientWelcome) -> (Cs2, Gc) {
    let (mut cs2, mut gc) = fake();
    let welcome = welcome(gc.account);
    let answer = async {
        gc.hello_read().await;
        gc.send(CLIENT_WELCOME, &welcome);
    };
    let (greeted, ()) = tokio::join!(soon(cs2.greet()), answer);
    greeted.unwrap();
    (cs2, gc)
}

/// A link to an item in the owner's inventory, leaked so that it outlives the `join!`s.
fn inventory(asset_id: u64) -> &'static InspectLink {
    Box::leak(Box::new(format!("S{OWNER}A{asset_id}D{D}").parse().unwrap()))
}

/// An item as the game coordinator describes it: an AK-47 Case Hardened with a sticker.
fn item(asset_id: u64) -> CEconItemPreviewDataBlock {
    let sticker = ProtoSticker { slot: Some(1), sticker_id: Some(5032), wear: Some(0.5), ..Default::default() };
    CEconItemPreviewDataBlock {
        itemid: Some(asset_id),
        defindex: Some(7),
        paintindex: Some(44),
        paintseed: Some(661),
        paintwear: Some(WEAR),
        stickers: vec![sticker],
        ..Default::default()
    }
}

fn answer(item: Option<CEconItemPreviewDataBlock>) -> CMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse {
    CMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse { iteminfo: item }
}

fn status(status: GcConnectionStatus) -> CMsgConnectionStatus {
    CMsgConnectionStatus { status: Some(status as i32), ..Default::default() }
}

fn fatal(code: u32, message: &str) -> CMsgGccStrike15V2ClientLogonFatalError {
    CMsgGccStrike15V2ClientLogonFatalError {
        errorcode: Some(code),
        message: Some(message.to_string()),
        ..Default::default()
    }
}

/// A message of type `msg_type` from CS2's game coordinator.
fn gc_message<M: Message>(msg_type: u32, body: &M) -> GcMessage {
    GcMessage::new(APP_ID, GcPacket::proto(msg_type, body))
}

/// Inspects `link` while the game coordinator answers the request with `response`. Returns the result and the
/// request.
async fn inspected(
    cs2: &mut Cs2,
    gc: &mut Gc,
    link: &InspectLink,
    response: CMsgGccStrike15V2Client2GcEconPreviewDataBlockResponse,
) -> (Result<ItemDetails>, CMsgGccStrike15V2Client2GcEconPreviewDataBlockRequest) {
    let answer = async {
        let (request, _) = gc.request_read().await;
        gc.send(PREVIEW_RESPONSE, &response);
        request
    };
    tokio::join!(soon(cs2.inspect(link)), answer)
}

fn stats(account_id: u32, level: i32) -> CMsgGccStrike15V2MatchmakingGc2ClientHello {
    CMsgGccStrike15V2MatchmakingGc2ClientHello {
        account_id: Some(account_id),
        player_level: Some(level),
        ..Default::default()
    }
}

fn profiles(profiles: Vec<CMsgGccStrike15V2MatchmakingGc2ClientHello>) -> CMsgGccStrike15V2PlayersProfile {
    CMsgGccStrike15V2PlayersProfile { account_profiles: profiles, ..Default::default() }
}

fn list(
    request: u32,
    account_id: Option<u32>,
    matches: Vec<CDataGccStrike15V2MatchInfo>,
) -> CMsgGccStrike15V2MatchList {
    CMsgGccStrike15V2MatchList { msgrequestid: Some(request), accountid: account_id, matches, ..Default::default() }
}

fn owner_soid(account: SteamId) -> CMsgSoidOwner {
    CMsgSoidOwner { r#type: Some(1), id: Some(account.as_u64()) }
}

/// A shared object holding `item`, of type `type_id`, owned by `owner` when it says.
fn object(type_id: i32, item: &CsoEconItem, owner: Option<SteamId>) -> CMsgSoSingleObject {
    CMsgSoSingleObject {
        type_id: Some(type_id),
        object_data: Some(items::object(item)),
        version: None,
        owner_soid: owner.map(owner_soid),
    }
}

/// A welcome with the account's cache, holding `items`, and someone else's.
fn welcome_with(account: SteamId, items: &[CsoEconItem]) -> CMsgClientWelcome {
    CMsgClientWelcome {
        outofdate_subscribed_caches: vec![
            items::cache(Some(SteamId::from_account_id(5)), &[items::item(99)]),
            items::cache(Some(account), items),
        ],
        ..Default::default()
    }
}

/// The account's cache at `version`, holding `items`.
fn cache_at(account: SteamId, version: u64, items: &[CsoEconItem]) -> CMsgSoCacheSubscribed {
    CMsgSoCacheSubscribed { version: Some(version), ..items::cache(Some(account), items) }
}

fn check(account: SteamId, version: u64) -> CMsgSoCacheSubscriptionCheck {
    CMsgSoCacheSubscriptionCheck { version: Some(version), owner_soid: Some(owner_soid(account)) }
}

fn ids(inventory: &Inventory) -> Vec<u64> {
    inventory.iter().map(InventoryItem::id).collect()
}

/// The ids of the current inventory; fails if it isn't current.
fn current_ids(cs2: &mut Cs2) -> Vec<u64> {
    ids(cs2.inventory().unwrap().expect("the inventory is current"))
}

/// The events waiting.
fn waiting(cs2: &mut Cs2) -> Vec<Cs2Event> {
    std::iter::from_fn(|| cs2.try_next_event().unwrap()).collect()
}

/// The events waiting, as [`summary`] has them.
fn drain(cs2: &mut Cs2) -> Vec<String> {
    summary(&waiting(cs2))
}

fn other_type(event: &Cs2Event) -> Option<u32> {
    match event {
        Cs2Event::Other(message) => Some(message.msg_type()),
        _ => None,
    }
}

/// The events, as short words: how the inventory changed and the session, with the item ids.
fn summary(events: &[Cs2Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            Cs2Event::SessionLost => "session lost".to_string(),
            Cs2Event::Welcomed => "welcomed".to_string(),
            Cs2Event::InventoryLoaded => "loaded".to_string(),
            Cs2Event::InventoryUnavailable => "unavailable".to_string(),
            Cs2Event::ItemAcquired(item) => format!("acquired {}", item.id()),
            Cs2Event::ItemChanged { new, .. } => format!("changed {}", new.id()),
            Cs2Event::ItemRemoved(item) => format!("removed {}", item.id()),
            Cs2Event::Lagged(missed) => format!("lagged {missed}"),
            Cs2Event::Other(message) => format!("other {}", message.msg_type()),
            Cs2Event::MatchmakingStats(_) => "stats".to_string(),
        })
        .collect()
}

/// A welcomed session, with the account's cache at version 10 holding item 1.
fn welcomed_with_version() -> (Cs2, Gc) {
    let (mut cs2, gc) = fake();
    cs2.session = super::Session::Welcomed;
    cs2.welcomed_before = true;
    gc.send(SO_CACHE_SUBSCRIBED, &cache_at(gc.account, 10, &[items::item(1)]));
    assert!(matches!(cs2.try_next_event(), Ok(Some(Cs2Event::InventoryLoaded))));
    assert_eq!(current_ids(&mut cs2), [1]);
    (cs2, gc)
}

// Greeting.

#[tokio::test(start_paused = true)]
async fn connects_once_welcomed_greeting_less_and_less_often() {
    let (mut cs2, mut gc) = fake();
    let start = Instant::now();
    let answer = async {
        let mut hellos = vec![];
        for _ in 0..3 {
            hellos.push(gc.hello_read().await - start);
        }
        gc.welcome();
        hellos
    };
    let (greeted, hellos) = tokio::join!(soon(cs2.greet()), answer);
    greeted.unwrap();
    assert!(cs2.has_session());
    // half a second, then 2 s, then 4 s
    assert_eq!(hellos, [Duration::from_millis(500), Duration::from_millis(2500), Duration::from_millis(6500)]);
    // welcomed: no more hellos
    gc.quiet(Duration::from_secs(120)).await;
    // the first welcome is no event
    assert!(cs2.try_next_event().unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn the_hellos_are_a_minute_apart_at_most() {
    let (mut cs2, mut gc) = fake();
    let answer = async {
        let mut hellos = vec![gc.hello_read().await];
        for _ in 0..8 {
            hellos.push(gc.hello_read().await);
        }
        // the first welcome is no event: what follows it is
        gc.welcome();
        gc.send(9999, &stats(1, 1));
        hellos.windows(2).map(|pair| (pair[1] - pair[0]).as_secs()).collect::<Vec<_>>()
    };
    // waiting for events greets without giving up
    let (event, gaps) = tokio::join!(soon(cs2.next_event()), answer);
    assert_eq!(event.as_ref().ok().and_then(other_type), Some(9999), "{event:?}");
    assert!(cs2.has_session());
    assert_eq!(gaps, [2, 4, 8, 16, 32, 60, 60, 60]);
}

#[tokio::test(start_paused = true)]
async fn connecting_times_out_without_a_welcome() {
    let (mut cs2, mut gc) = fake();
    let start = Instant::now();
    assert!(matches!(soon(cs2.greet()).await, Err(Error::Timeout(CLIENT_WELCOME))));
    assert_eq!(start.elapsed(), GREET_TIMEOUT);
    // at 0.5, 2.5, 6.5, 14.5 and 30.5 s
    for _ in 0..5 {
        gc.hello_read().await;
    }
    assert!(gc.try_to_gc().is_none());
}

#[tokio::test(start_paused = true)]
async fn connecting_fails_when_the_game_coordinator_refuses_the_session() {
    let (mut cs2, mut gc) = fake();
    let answer = async {
        gc.hello_read().await;
        gc.send(LOGON_FATAL_ERROR, &fatal(4, "No CS2 license"));
    };
    let (result, ()) = tokio::join!(soon(cs2.greet()), answer);
    match result {
        Err(Error::Refused { code, message }) => assert_eq!((code, message.as_str()), (4, "No CS2 license")),
        result => panic!("expected Refused, got {result:?}"),
    }
}

// Inspecting.

#[tokio::test(start_paused = true)]
async fn inspects_an_item_in_an_inventory() {
    let (mut cs2, mut gc) = session().await;
    let (details, request) = inspected(&mut cs2, &mut gc, inventory(ASSET), answer(Some(item(ASSET)))).await;
    assert_eq!((request.param_s(), request.param_a(), request.param_d(), request.param_m()), (OWNER, ASSET, D, 0));
    let details = details.unwrap();
    assert_eq!(details.item_id(), Some(ASSET));
    assert_eq!(details.def_index(), 7);
    assert_eq!(details.paint_index(), Some(44));
    assert_eq!(details.pattern(), Some(661));
    assert!((details.float().unwrap() - 0.204_828_55).abs() < 1e-7);
    let stickers: Vec<_> = details.stickers().map(|sticker| (sticker.slot, sticker.id, sticker.wear)).collect();
    assert_eq!(stickers, [(1, 5032, Some(0.5))]);
}

#[tokio::test(start_paused = true)]
async fn skips_answers_about_other_items() {
    let (mut cs2, mut gc) = session().await;
    let listing = 625254122282020305;
    let asset = 6760346663;
    let link: InspectLink = format!("M{listing}A{asset}D{D}").parse().unwrap();
    let answers = async {
        let (request, _) = gc.request_read().await;
        assert_eq!(
            (request.param_s(), request.param_a(), request.param_d(), request.param_m()),
            (0, asset, D, listing)
        );
        // about another item, then about none it seems, then the one asked
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET))));
        gc.send(PREVIEW_RESPONSE, &answer(Some(CEconItemPreviewDataBlock::default())));
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(asset))));
    };
    let (details, ()) = tokio::join!(soon(cs2.inspect(&link)), answers);
    assert_eq!(details.unwrap().item_id(), Some(asset));
    let others: Vec<_> = waiting(&mut cs2).iter().map(other_type).collect();
    assert_eq!(others, [Some(PREVIEW_RESPONSE), Some(PREVIEW_RESPONSE)]);
}

#[tokio::test(start_paused = true)]
async fn an_inspect_times_out_and_its_late_answer_is_skipped() {
    let (mut cs2, mut gc) = session().await;
    let (result, (_, sent)) = tokio::join!(soon(cs2.inspect(inventory(ASSET))), gc.request_read());
    assert!(matches!(result, Err(Error::Timeout(PREVIEW_RESPONSE))));
    assert_eq!(sent.elapsed(), REQUEST_TIMEOUT);

    let answers = async {
        let (request, _) = gc.request_read().await;
        assert_eq!(request.param_a(), ASSET + 1);
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET))));
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET + 1))));
    };
    let (details, ()) = tokio::join!(soon(cs2.inspect(inventory(ASSET + 1))), answers);
    assert_eq!(details.unwrap().item_id(), Some(ASSET + 1));
}

#[tokio::test(start_paused = true)]
async fn the_timeout_can_be_changed() {
    let (cs2, mut gc) = session().await;
    let mut cs2 = cs2.with_timeout(Duration::from_secs(2));
    let (result, (_, sent)) = tokio::join!(soon(cs2.inspect(inventory(ASSET))), gc.request_read());
    assert!(matches!(result, Err(Error::Timeout(PREVIEW_RESPONSE))));
    assert_eq!(sent.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn waits_the_cooldown_from_one_request_to_the_next() {
    let (mut cs2, mut gc) = session().await;
    let answers = async {
        let mut sent = vec![];
        for asset in 0..2 {
            let (_, at) = gc.request_read().await;
            sent.push(at);
            gc.send(PREVIEW_RESPONSE, &answer(Some(item(asset))));
        }
        // answered after the cooldown: the next request goes at once
        let (_, at) = gc.request_read().await;
        sent.push(at);
        tokio::time::sleep(Duration::from_secs(2)).await;
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(2))));
        let (_, at) = gc.request_read().await;
        sent.push(at);
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(3))));
        sent
    };
    let inspects = async {
        for asset in 0..4 {
            soon(cs2.inspect(inventory(asset))).await.unwrap();
        }
    };
    let (sent, ()) = tokio::join!(answers, inspects);
    let gaps: Vec<_> = sent.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(gaps, [REQUEST_COOLDOWN, REQUEST_COOLDOWN, Duration::from_secs(2)]);
}

#[tokio::test(start_paused = true)]
async fn the_cooldown_can_be_changed() {
    let (cs2, mut gc) = session().await;
    let mut cs2 = cs2.with_cooldown(Duration::from_secs(3));
    let answers = async {
        let mut sent = vec![];
        for asset in 0..2 {
            let (_, at) = gc.request_read().await;
            sent.push(at);
            gc.send(PREVIEW_RESPONSE, &answer(Some(item(asset))));
        }
        sent[1] - sent[0]
    };
    let inspects = async {
        for asset in 0..2 {
            soon(cs2.inspect(inventory(asset))).await.unwrap();
        }
    };
    let (gap, ()) = tokio::join!(answers, inspects);
    assert_eq!(gap, Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn greets_again_when_the_game_coordinator_has_no_session() {
    let (mut cs2, mut gc) = session().await;
    // the status `NoSession`, then one the crate doesn't know: neither is a session
    for lost in [Some(GcConnectionStatus::NoSession), None] {
        match lost {
            Some(lost) => gc.send(CONNECTION_STATUS, &status(lost)),
            None => gc.send(CONNECTION_STATUS, &CMsgConnectionStatus { status: Some(9), ..Default::default() }),
        }
        let answers = async {
            gc.hello_read().await;
            gc.welcome();
            let (request, _) = gc.request_read().await;
            gc.send(PREVIEW_RESPONSE, &answer(Some(item(request.param_a()))));
        };
        let (details, ()) = tokio::join!(soon(cs2.inspect(inventory(ASSET))), answers);
        details.unwrap();
        assert!(cs2.has_session());
    }
}

#[tokio::test(start_paused = true)]
async fn a_session_kept_or_welcomed_again_unasked_needs_no_greeting() {
    let (mut cs2, mut gc) = session().await;
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::HaveSession));
    // the default status is a session
    gc.send(CONNECTION_STATUS, &CMsgConnectionStatus::default());
    // straight to the request
    let (details, _) = inspected(&mut cs2, &mut gc, inventory(ASSET), answer(Some(item(ASSET)))).await;
    details.unwrap();

    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::GcGoingDown));
    gc.welcome();
    let (details, _) = inspected(&mut cs2, &mut gc, inventory(ASSET), answer(Some(item(ASSET)))).await;
    details.unwrap();
}

#[tokio::test(start_paused = true)]
async fn embedded_links_are_read_without_the_game_coordinator() {
    let (mut cs2, mut gc) = session().await;
    let start = Instant::now();
    let details = soon(cs2.inspect(&EMBEDDED.parse().unwrap())).await.unwrap();
    assert_eq!(details.def_index(), 60);
    assert_eq!(details.item_id(), None);
    assert_eq!(start.elapsed(), Duration::ZERO);
    gc.quiet(Duration::from_secs(30)).await;

    // a link that isn't one
    assert!(matches!(soon(cs2.inspect_str("S1A2")).await, Err(Error::InspectLink(InspectLinkError::Missing('D')))));
    let details = soon(cs2.inspect_str(EMBEDDED)).await.unwrap();
    assert_eq!(details.def_index(), 60);
}

#[tokio::test(start_paused = true)]
async fn an_answer_without_an_item_is_an_error() {
    let (mut cs2, mut gc) = session().await;
    let (result, _) = inspected(&mut cs2, &mut gc, inventory(ASSET), answer(None)).await;
    assert!(matches!(result, Err(Error::ItemDetails(ItemDetailsError::NoItem))));
    // an item without a def index isn't an item
    let nameless = CEconItemPreviewDataBlock { defindex: None, ..item(ASSET) };
    let (result, _) = inspected(&mut cs2, &mut gc, inventory(ASSET), answer(Some(nameless))).await;
    assert!(matches!(result, Err(Error::ItemDetails(ItemDetailsError::NoDefIndex))));
}

#[tokio::test(start_paused = true)]
async fn requests_fail_once_the_client_disconnected() {
    let (mut cs2, mut gc) = session().await;
    gc.disconnect();
    assert!(matches!(soon(cs2.inspect(inventory(ASSET))).await, Err(Error::Closed)));
    assert!(!cs2.has_session());
    // and again, without greeting
    assert!(matches!(soon(cs2.live_matches()).await, Err(Error::Closed)));
    assert!(gc.try_to_gc().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_disconnection_while_waiting_ends_the_request() {
    let (mut cs2, mut gc) = session().await;
    let disconnect = async {
        gc.request_read().await;
        gc.disconnect();
    };
    let (result, ()) = tokio::join!(soon(cs2.inspect(inventory(ASSET))), disconnect);
    assert!(matches!(result, Err(Error::Closed)));
    assert_eq!(summary(&[cs2.try_next_event().unwrap().unwrap()]), ["session lost"]);
    assert_eq!(summary(&[cs2.try_next_event().unwrap().unwrap()]), ["unavailable"]);
    assert!(matches!(cs2.try_next_event(), Err(Error::Closed)));
}

// Other requests.

#[tokio::test(start_paused = true)]
async fn keeps_the_inventory_and_the_stats_the_game_coordinator_sends() {
    let (mut cs2, gc) = session_welcomed(|account| welcome_with(account, &[items::item(1), items::item(2)])).await;
    // the account's items only
    assert_eq!(current_ids(&mut cs2), [1, 2]);
    let item = cs2.inventory().unwrap().unwrap().get(1).unwrap();
    assert_eq!((item.paint_index(), item.pattern()), (Some(44), Some(661)));
    assert!(cs2.matchmaking_stats().is_none());

    gc.send(MATCHMAKING_HELLO, &stats(PLAYER_ACCOUNT, 12));
    // the first welcome is no event
    match waiting(&mut cs2).as_slice() {
        [Cs2Event::MatchmakingStats(stats)] => assert_eq!(stats.level(), Some(12)),
        events => panic!("unexpected events: {events:?}"),
    }
    assert_eq!(cs2.matchmaking_stats().unwrap().level(), Some(12));
    assert!(cs2.has_session());
}

#[tokio::test(start_paused = true)]
async fn asks_for_the_matchmaking_stats_again() {
    let (mut cs2, mut gc) = session().await;
    let answers = async {
        let (request, _) =
            gc.read_request::<CMsgGccStrike15V2MatchmakingClient2GcHello>(MATCHMAKING_CLIENT_HELLO).await;
        assert_eq!(request.encoded_len(), 0);
        gc.send(MATCHMAKING_HELLO, &stats(PLAYER_ACCOUNT, 30));
    };
    let (stats, ()) = tokio::join!(soon(cs2.request_matchmaking_stats()), answers);
    assert_eq!(stats.unwrap().level(), Some(30));
    assert_eq!(cs2.matchmaking_stats().unwrap().level(), Some(30));
    // the answer is no event
    assert!(waiting(&mut cs2).is_empty());
}

#[tokio::test(start_paused = true)]
async fn asks_for_a_players_profile() {
    let (mut cs2, mut gc) = session().await;
    let answers = async {
        let (request, _) =
            gc.read_request::<CMsgGccStrike15V2ClientRequestPlayersProfile>(REQUEST_PLAYERS_PROFILE).await;
        assert_eq!((request.account_id(), request.request_level()), (PLAYER_ACCOUNT, 32));
        // none, someone else's, then the player's
        gc.send(PLAYERS_PROFILE, &profiles(vec![]));
        gc.send(PLAYERS_PROFILE, &profiles(vec![stats(7, 1)]));
        gc.send(PLAYERS_PROFILE, &profiles(vec![stats(PLAYER_ACCOUNT, 21), stats(7, 1)]));
    };
    let (profile, ()) = tokio::join!(soon(cs2.player_profile(SteamId::from_u64(PLAYER))), answers);
    let profile = profile.unwrap();
    assert_eq!(profile.player(), Some(SteamId::from_u64(PLAYER)));
    assert_eq!(profile.level(), Some(21));
    // the others are passed on
    let others: Vec<_> = waiting(&mut cs2).iter().map(other_type).collect();
    assert_eq!(others, [Some(PLAYERS_PROFILE), Some(PLAYERS_PROFILE)]);
}

#[tokio::test(start_paused = true)]
async fn requests_about_players_need_individual_accounts() {
    let (mut cs2, mut gc) = session().await;
    let clan = SteamId::from_u64(103582791429521412);
    assert!(matches!(soon(cs2.player_profile(clan)).await, Err(Error::NotIndividual(id)) if id == clan));
    let nobody = SteamId::from_account_id(0);
    assert!(matches!(soon(cs2.recent_matches(nobody)).await, Err(Error::NotIndividual(_))));
    assert!(matches!(soon(cs2.live_match(clan)).await, Err(Error::NotIndividual(_))));
    gc.quiet(Duration::from_secs(30)).await;
}

#[tokio::test(start_paused = true)]
async fn a_profile_request_times_out() {
    let (mut cs2, mut gc) = session().await;
    let player = SteamId::from_u64(PLAYER);
    let (result, (_, sent)) = tokio::join!(
        soon(cs2.player_profile(player)),
        gc.read_request::<CMsgGccStrike15V2ClientRequestPlayersProfile>(REQUEST_PLAYERS_PROFILE)
    );
    assert!(matches!(result, Err(Error::Timeout(PLAYERS_PROFILE))));
    assert_eq!(sent.elapsed(), REQUEST_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn asks_for_a_match_by_its_share_code() {
    let (mut cs2, mut gc) = session().await;
    let code: ShareCode = SHARE_CODE.parse().unwrap();
    let answers = async {
        let (request, _) = gc.read_request::<CMsgGccStrike15V2MatchListRequestFullGameInfo>(FULL_GAME_INFO).await;
        assert_eq!(
            (request.matchid(), request.outcomeid(), request.token()),
            (SHARED_MATCH, 3400367402569957763, 9725)
        );
        // the answer to another request, about another match, then the one asked
        gc.send(MATCH_LIST, &list(RECENT_GAMES, Some(PLAYER_ACCOUNT), vec![finished(SHARED_MATCH)]));
        gc.send(MATCH_LIST, &list(FULL_GAME_INFO, None, vec![finished(1)]));
        gc.send(MATCH_LIST, &list(FULL_GAME_INFO, None, vec![finished(SHARED_MATCH)]));
    };
    let (matches, ()) = tokio::join!(soon(cs2.match_info(&code)), answers);
    let matches = matches.unwrap();
    assert_eq!(matches.raw().msgrequestid(), FULL_GAME_INFO);
    let info = matches.matches().next().unwrap();
    assert_eq!(info.match_id(), Some(SHARED_MATCH));
    assert_eq!(info.share_code(), Some(code));
    assert!(info.demo_url().unwrap().ends_with(".dem.bz2"));
    let others: Vec<_> = waiting(&mut cs2).iter().map(other_type).collect();
    assert_eq!(others, [Some(MATCH_LIST), Some(MATCH_LIST)]);

    // the game coordinator found none
    let answers = async {
        gc.read_request::<CMsgGccStrike15V2MatchListRequestFullGameInfo>(FULL_GAME_INFO).await;
        gc.send(MATCH_LIST, &list(FULL_GAME_INFO, None, vec![]));
    };
    let (matches, ()) = tokio::join!(soon(cs2.match_info(&code)), answers);
    assert_eq!(matches.unwrap().matches().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn asks_for_recent_and_live_matches() {
    let (mut cs2, mut gc) = session().await;
    let player = SteamId::from_u64(PLAYER);
    let answers = async {
        let (request, _) = gc.read_request::<CMsgGccStrike15V2MatchListRequestRecentUserGames>(RECENT_GAMES).await;
        assert_eq!(request.accountid(), PLAYER_ACCOUNT);
        // about someone else, then the player
        gc.send(MATCH_LIST, &list(RECENT_GAMES, Some(7), vec![finished(3)]));
        gc.send(MATCH_LIST, &list(RECENT_GAMES, Some(PLAYER_ACCOUNT), vec![finished(1), finished(2)]));
    };
    let (recent, ()) = tokio::join!(soon(cs2.recent_matches(player)), answers);
    let recent = recent.unwrap();
    assert_eq!(recent.player(), Some(player));
    let ids: Vec<_> = recent.matches().map(|info| info.match_id().unwrap()).collect();
    assert_eq!(ids, [1, 2]);

    let answers = async {
        let (request, _) = gc.read_request::<CMsgGccStrike15V2MatchListRequestCurrentLiveGames>(LIVE_GAMES).await;
        assert_eq!(request.encoded_len(), 0);
        gc.send(MATCH_LIST, &list(LIVE_GAMES, None, vec![finished(4)]));
    };
    let (live, ()) = tokio::join!(soon(cs2.live_matches()), answers);
    assert_eq!(live.unwrap().matches().len(), 1);

    // which the game coordinator doesn't answer any more, ValvePython says
    let answers = async {
        let (request, _) =
            gc.read_request::<CMsgGccStrike15V2MatchListRequestLiveGameForUser>(LIVE_GAME_FOR_USER).await;
        assert_eq!(request.accountid(), PLAYER_ACCOUNT);
        gc.send(MATCH_LIST, &list(LIVE_GAMES, None, vec![]));
    };
    let (live, ()) = tokio::join!(soon(cs2.live_match(player)), answers);
    assert!(matches!(live, Err(Error::Timeout(MATCH_LIST))));
}

#[tokio::test(start_paused = true)]
async fn every_kind_of_request_waits_the_same_cooldown() {
    let (mut cs2, mut gc) = session().await;
    let answers = async {
        let (_, inspected) = gc.request_read().await;
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET))));
        let (_, profiled) =
            gc.read_request::<CMsgGccStrike15V2ClientRequestPlayersProfile>(REQUEST_PLAYERS_PROFILE).await;
        gc.send(PLAYERS_PROFILE, &profiles(vec![stats(PLAYER_ACCOUNT, 1)]));
        let (_, listed) = gc.read_request::<CMsgGccStrike15V2MatchListRequestCurrentLiveGames>(LIVE_GAMES).await;
        gc.send(MATCH_LIST, &list(LIVE_GAMES, None, vec![]));
        [profiled - inspected, listed - profiled]
    };
    let requests = async {
        soon(cs2.inspect(inventory(ASSET))).await.unwrap();
        soon(cs2.player_profile(SteamId::from_u64(PLAYER))).await.unwrap();
        soon(cs2.live_matches()).await.unwrap();
    };
    let (gaps, ()) = tokio::join!(answers, requests);
    assert_eq!(gaps, [REQUEST_COOLDOWN, REQUEST_COOLDOWN]);
}

#[tokio::test(start_paused = true)]
async fn keeps_what_comes_while_a_request_waits() {
    let (mut cs2, mut gc) = session().await;
    let account = gc.account;
    let answers = async {
        gc.request_read().await;
        gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(5), Some(account)));
        gc.send(9999, &stats(1, 1));
        gc.send(MATCHMAKING_HELLO, &stats(PLAYER_ACCOUNT, 3));
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET + 1))));
        gc.send(PREVIEW_RESPONSE, &answer(Some(item(ASSET))));
    };
    let (details, ()) = tokio::join!(soon(cs2.inspect(inventory(ASSET))), answers);
    assert_eq!(details.unwrap().item_id(), Some(ASSET));
    assert_eq!(current_ids(&mut cs2), [5]);
    match waiting(&mut cs2).as_slice() {
        [Cs2Event::ItemAcquired(item), Cs2Event::Other(unknown), Cs2Event::MatchmakingStats(stats), Cs2Event::Other(late)] =>
        {
            assert_eq!(item.id(), 5);
            assert_eq!(unknown.msg_type(), 9999);
            assert_eq!(stats.level(), Some(3));
            assert_eq!(late.msg_type(), PREVIEW_RESPONSE);
        }
        events => panic!("unexpected events: {events:?}"),
    }
}

// Events and the inventory.

#[tokio::test(start_paused = true)]
async fn follows_the_inventory() {
    let (mut cs2, gc) = session_welcomed(|account| welcome_with(account, &[items::item(1), items::item(2)])).await;
    let account = gc.account;
    let moved = CsoEconItem { inventory: Some(8), ..items::item(1) };
    let gone = CsoEconItem { id: Some(2), ..Default::default() };
    let unknown = CsoEconItem { id: Some(77), ..Default::default() };
    gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(3), Some(account)));
    gc.send(SO_UPDATE, &object(ITEM_TYPE, &moved, None));
    gc.send(SO_DESTROY, &object(ITEM_TYPE, &gone, Some(account)));
    // nothing to remove; nothing changed
    gc.send(SO_DESTROY, &object(ITEM_TYPE, &unknown, Some(account)));
    gc.send(SO_UPDATE, &object(ITEM_TYPE, &moved, None));
    // someone else's, and not an item: passed on
    gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(4), Some(SteamId::from_account_id(5))));
    gc.send(SO_CREATE, &object(7, &items::item(4), None));
    match waiting(&mut cs2).as_slice() {
        [Cs2Event::ItemAcquired(acquired), Cs2Event::ItemChanged { old, new }, Cs2Event::ItemRemoved(removed), Cs2Event::Other(someones), Cs2Event::Other(not_item)] =>
        {
            assert_eq!(acquired.id(), 3);
            assert_eq!((old.id(), old.position(), new.position()), (1, Some(3), Some(8)));
            // the item as it was: the message only names it
            assert_eq!((removed.id(), removed.paint_index()), (2, Some(44)));
            assert_eq!((someones.msg_type(), not_item.msg_type()), (SO_CREATE, SO_CREATE));
        }
        events => panic!("unexpected events: {events:?}"),
    }
    assert_eq!(current_ids(&mut cs2), [1, 3]);
    assert_eq!(cs2.inventory().unwrap().unwrap().get(1).unwrap().position(), Some(8));

    // the whole inventory again: what changed, then the load
    gc.send(SO_CACHE_SUBSCRIBED, &items::cache(Some(account), &[items::item(6)]));
    // several objects at once
    let objects = CMsgSoMultipleObjects {
        objects_modified: vec![SingleObject {
            type_id: Some(ITEM_TYPE),
            object_data: Some(items::object(&items::item(7))),
        }],
        ..Default::default()
    };
    gc.send(SO_UPDATE_MULTIPLE, &objects);
    assert_eq!(drain(&mut cs2), ["removed 1", "removed 3", "acquired 6", "loaded", "acquired 7"]);
    assert_eq!(current_ids(&mut cs2), [6, 7]);
}

#[tokio::test(start_paused = true)]
async fn passes_on_messages_it_doesnt_read() {
    let (mut cs2, gc) = session().await;
    // `ItemCustomizationNotification`
    gc.send(1090, &stats(1, 1));
    match soon(cs2.next_event()).await.unwrap() {
        Cs2Event::Other(message) => {
            assert_eq!(message.msg_type(), 1090);
            assert!(message.is_proto());
            let body = message.packet.decode::<CMsgGccStrike15V2MatchmakingGc2ClientHello>().unwrap();
            assert_eq!(body.player_level(), 1);
        }
        event => panic!("unexpected event: {event:?}"),
    }
    assert!(cs2.try_next_event().unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_status_without_session_is_an_event_then_a_greeting() {
    let (mut cs2, mut gc) = session().await;
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::NoSession));
    // an unasked welcome
    gc.welcome();
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::GcGoingDown));
    assert_eq!(drain(&mut cs2), ["session lost", "unavailable", "welcomed", "session lost"]);
    assert!(!cs2.has_session());

    let answers = async {
        gc.hello_read().await;
        gc.send(LOGON_FATAL_ERROR, &fatal(4, ""));
    };
    let (refused, ()) = tokio::join!(soon(cs2.next_event()), answers);
    assert!(matches!(refused, Err(Error::Refused { code: 4, .. })));
    // greets again at the next call, from the first hello
    let answers = async {
        let start = Instant::now();
        gc.hello_read().await;
        gc.welcome();
        start.elapsed()
    };
    let (event, first) = tokio::join!(soon(cs2.next_event()), answers);
    assert!(matches!(event, Ok(Cs2Event::Welcomed)));
    assert_eq!(first, FIRST_HELLO);
}

#[tokio::test(start_paused = true)]
async fn greets_again_while_waiting_for_events() {
    let (mut cs2, mut gc) = session_welcomed(|account| welcome_with(account, &[items::item(1)])).await;
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::NoSession));
    assert!(matches!(soon(cs2.next_event()).await, Ok(Cs2Event::SessionLost)));
    assert!(matches!(soon(cs2.next_event()).await, Ok(Cs2Event::InventoryUnavailable)));

    let account = gc.account;
    let answers = async {
        let start = Instant::now();
        gc.hello_read().await;
        let first = start.elapsed();
        gc.hello_read().await;
        gc.send(CLIENT_WELCOME, &welcome_with(account, &[items::item(4)]));
        (first, start.elapsed())
    };
    let (event, hellos) = tokio::join!(soon(cs2.next_event()), answers);
    assert!(matches!(event, Ok(Cs2Event::Welcomed)));
    assert_eq!(hellos, (Duration::from_millis(500), Duration::from_millis(2500)));
    assert!(cs2.has_session());
    assert_eq!(current_ids(&mut cs2), [4]);
    assert_eq!(drain(&mut cs2), ["removed 1", "acquired 4", "loaded"]);
    // welcomed: waits without greeting
    let wait = timeout(Duration::from_secs(120), cs2.next_event());
    let (event, ()) = tokio::join!(wait, gc.quiet(Duration::from_secs(110)));
    assert!(event.is_err());
}

#[tokio::test(start_paused = true)]
async fn the_events_end_once_the_client_disconnected() {
    let (mut cs2, mut gc) = session_welcomed(|account| welcome_with(account, &[items::item(1)])).await;
    gc.send(9999, &stats(1, 1));
    gc.disconnect();
    // what came first, then the end of the session, then the end
    assert_eq!(other_type(&soon(cs2.next_event()).await.unwrap()), Some(9999));
    assert!(matches!(soon(cs2.next_event()).await, Ok(Cs2Event::SessionLost)));
    assert!(matches!(soon(cs2.next_event()).await, Ok(Cs2Event::InventoryUnavailable)));
    assert!(matches!(soon(cs2.next_event()).await, Err(Error::Closed)));
    assert!(matches!(cs2.try_next_event(), Err(Error::Closed)));
    assert!(matches!(cs2.inventory(), Err(Error::Closed)));
    // it doesn't greet a game coordinator it can't hear
    assert!(gc.try_to_gc().is_none());
}

#[tokio::test(start_paused = true)]
async fn try_next_event_reads_the_disconnection_as_events_first() {
    let (mut cs2, mut gc) = session().await;
    gc.disconnect();
    assert_eq!(summary(&[cs2.try_next_event().unwrap().unwrap()]), ["session lost"]);
    assert_eq!(summary(&[cs2.try_next_event().unwrap().unwrap()]), ["unavailable"]);
    assert!(matches!(cs2.try_next_event(), Err(Error::Closed)));
}

#[test]
fn keeps_events_up_to_the_capacity() {
    let mut events = EventQueue::default();
    let message = gc_message(9999, &stats(1, 1));
    for _ in 0..EVENT_CAPACITY + 2 {
        events.push(Cs2Event::Other(message.clone()));
    }
    // about the session: never dropped, one is enough
    events.push(Cs2Event::SessionLost);
    events.push(Cs2Event::SessionLost);
    events.push(Cs2Event::Welcomed);
    // still full
    events.push(Cs2Event::Other(message.clone()));
    let mut others = 0;
    while let Some(Cs2Event::Other(_)) = events.events.front() {
        events.pop();
        others += 1;
    }
    assert_eq!(others, EVENT_CAPACITY);
    let rest: Vec<_> = std::iter::from_fn(|| events.pop()).collect();
    assert_eq!(summary(&rest), ["lagged 2", "session lost", "welcomed", "lagged 1"]);
    assert!(events.is_empty());
}

#[test]
fn inventory_events_are_never_dropped() {
    let mut events = EventQueue::default();
    let message = gc_message(9999, &stats(1, 1));
    for _ in 0..EVENT_CAPACITY + 1 {
        events.push(Cs2Event::Other(message.clone()));
    }
    events.push(Cs2Event::InventoryUnavailable);
    events.push(Cs2Event::InventoryUnavailable);
    events.push(Cs2Event::InventoryLoaded);
    let rest: Vec<_> = std::iter::from_fn(|| events.pop()).skip(EVENT_CAPACITY).collect();
    assert_eq!(summary(&rest), ["lagged 1", "unavailable", "loaded"]);
}

// The inventory is current or none, never an old one.

#[tokio::test(start_paused = true)]
async fn the_inventory_is_none_once_the_session_ends() {
    let (mut cs2, gc) = session_welcomed(|account| welcome_with(account, &[items::item(1)])).await;
    assert_eq!(current_ids(&mut cs2), [1]);
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::NoSession));
    // read by `inventory` itself, without waiting for an event
    assert!(cs2.inventory().unwrap().is_none());
    // what the game coordinator sends without a session changes nothing, and isn't passed on
    gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(5), Some(gc.account)));
    gc.send(SO_CACHE_SUBSCRIBED, &cache_at(gc.account, 3, &[items::item(5)]));
    assert_eq!(drain(&mut cs2), ["session lost", "unavailable"]);
    assert!(cs2.inventory().unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_cache_without_items_is_an_empty_inventory() {
    let (mut cs2, gc) = session_welcomed(|account| welcome_with(account, &[items::item(1), items::item(2)])).await;
    // the account's cache, holding objects of other types only: every item is gone
    let mut cache = items::cache(Some(gc.account), &[]);
    cache.objects.retain(|objects| objects.type_id() != ITEM_TYPE);
    gc.send(SO_CACHE_SUBSCRIBED, &cache);
    assert_eq!(drain(&mut cs2), ["removed 1", "removed 2", "loaded"]);
    assert!(current_ids(&mut cs2).is_empty());

    // a welcome whose cache holds no items, the same
    let (mut cs2, _gc) = session_welcomed(|account| {
        let mut welcome = welcome_with(account, &[]);
        welcome.outofdate_subscribed_caches[1].objects.clear();
        welcome
    })
    .await;
    assert!(current_ids(&mut cs2).is_empty());
}

#[tokio::test(start_paused = true)]
async fn what_changed_while_the_inventory_was_none_comes_as_events() {
    let first = CsoEconItem { inventory: Some(3), ..items::item(2) };
    let (mut cs2, mut gc) = session_welcomed(|account| welcome_with(account, &[items::item(1), first.clone()])).await;
    let account = gc.account;
    gc.send(CONNECTION_STATUS, &status(GcConnectionStatus::NoSession));
    let moved = CsoEconItem { inventory: Some(9), ..first };
    let answers = async {
        gc.hello_read().await;
        // before the welcome: no session yet, so nothing
        gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(8), Some(account)));
        gc.send(CLIENT_WELCOME, &welcome_with(account, &[moved, items::item(3)]));
    };
    let events = async {
        let mut events = vec![];
        loop {
            let event = cs2.next_event().await.unwrap();
            let loaded = matches!(event, Cs2Event::InventoryLoaded);
            events.push(event);
            if loaded {
                return events;
            }
        }
    };
    let (events, ()) = tokio::join!(soon(events), answers);
    assert_eq!(
        summary(&events),
        ["session lost", "unavailable", "welcomed", "removed 1", "changed 2", "acquired 3", "loaded"]
    );
    assert_eq!(current_ids(&mut cs2), [2, 3]);
    assert_eq!(cs2.inventory().unwrap().unwrap().get(2).unwrap().position(), Some(9));
}

#[tokio::test(start_paused = true)]
async fn the_game_coordinator_can_say_the_inventory_is_old_or_gone() {
    let (mut cs2, mut gc) = welcomed_with_version();
    let account = gc.account;
    // the same version: still current
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(account, 10));
    assert!(drain(&mut cs2).is_empty());
    assert_eq!(current_ids(&mut cs2), [1]);
    assert!(gc.try_to_gc().is_none());

    // a change moves the version
    let created = CMsgSoSingleObject { version: Some(11), ..object(ITEM_TYPE, &items::item(2), Some(account)) };
    gc.send(SO_CREATE, &created);
    // so do changes to other objects of the cache
    let other = CMsgSoSingleObject { version: Some(12), ..object(7, &items::item(3), Some(account)) };
    gc.send(SO_UPDATE, &other);
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(account, 12));
    assert_eq!(drain(&mut cs2), ["acquired 2", "other 22"]);
    assert_eq!(current_ids(&mut cs2), [1, 2]);
    // several at once, too
    let objects = CMsgSoMultipleObjects {
        objects_modified: vec![SingleObject { type_id: Some(7), object_data: None }],
        version: Some(13),
        ..Default::default()
    };
    gc.send(SO_UPDATE_MULTIPLE, &objects);
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(account, 13));
    assert_eq!(drain(&mut cs2), ["other 26"]);
    assert_eq!(current_ids(&mut cs2), [1, 2]);
    assert!(gc.try_to_gc().is_none());

    // someone else's check isn't about the inventory
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(SteamId::from_account_id(5), 1));
    assert_eq!(drain(&mut cs2), ["other 27"]);

    // another version: old, asks for it again
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(account, 14));
    assert_eq!(drain(&mut cs2), ["unavailable"]);
    assert!(cs2.inventory().unwrap().is_none());
    gc.assert_refresh_sent();
    // a check while none asks again
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(account, 14));
    assert!(drain(&mut cs2).is_empty());
    gc.assert_refresh_sent();
    gc.send(SO_CACHE_SUBSCRIBED, &cache_at(account, 14, &[items::item(2)]));
    assert_eq!(drain(&mut cs2), ["removed 1", "loaded"]);

    // the cache unsubscribed: none, until it comes again
    let mut unsubscribed = CMsgSoCacheUnsubscribed { owner_soid: Some(owner_soid(account)) };
    gc.send(SO_CACHE_UNSUBSCRIBED, &unsubscribed);
    assert_eq!(drain(&mut cs2), ["unavailable"]);
    assert!(cs2.inventory().unwrap().is_none());
    assert!(gc.try_to_gc().is_none());
    // someone else's isn't ours
    unsubscribed.owner_soid = Some(owner_soid(SteamId::from_account_id(5)));
    gc.send(SO_CACHE_UNSUBSCRIBED, &unsubscribed);
    assert_eq!(drain(&mut cs2), ["other 25"]);
}

#[tokio::test(start_paused = true)]
async fn a_welcome_without_the_inventory_leaves_none() {
    let (mut cs2, mut gc) = welcomed_with_version();
    let account = gc.account;
    // the game coordinator restarted: a new session, without the account's cache
    gc.welcome();
    assert_eq!(drain(&mut cs2), ["welcomed", "unavailable"]);
    assert!(cs2.inventory().unwrap().is_none());
    assert!(gc.try_to_gc().is_none());
    // one that says the account's is up to date, which it can't be: asks for it
    let welcome = CMsgClientWelcome { uptodate_subscribed_caches: vec![check(account, 10)], ..Default::default() };
    gc.send(CLIENT_WELCOME, &welcome);
    assert_eq!(drain(&mut cs2), ["welcomed"]);
    gc.assert_refresh_sent();
    assert!(cs2.inventory().unwrap().is_none());
    // an unreadable welcome is a session without it, too
    gc.send(SO_CACHE_SUBSCRIBED, &cache_at(account, 1, &[]));
    assert_eq!(drain(&mut cs2), ["removed 1", "loaded"]);
    gc.push(GcMessage::new(
        APP_ID,
        GcPacket { body: Bytes::from_static(b"\x0a"), ..GcPacket::proto(CLIENT_WELCOME, &()) },
    ));
    assert_eq!(drain(&mut cs2), ["welcomed", "unavailable"]);
}

#[tokio::test(start_paused = true)]
async fn what_cant_be_read_makes_the_inventory_none_until_it_comes_again() {
    let (mut cs2, mut gc) = welcomed_with_version();
    let account = gc.account;
    let reload = |cs2: &mut Cs2, gc: &Gc| {
        gc.send(SO_CACHE_SUBSCRIBED, &cache_at(account, 20, &[items::item(1)]));
        assert_eq!(drain(cs2).last().map(String::as_str), Some("loaded"));
        assert_eq!(current_ids(cs2), [1]);
    };
    // an update of an item it can't read: the copy would keep it as it was
    let unreadable = CsoEconItem { def_index: None, ..items::item(1) };
    gc.send(SO_UPDATE, &object(ITEM_TYPE, &unreadable, Some(account)));
    assert_eq!(drain(&mut cs2), ["unavailable"]);
    assert!(cs2.inventory().unwrap().is_none());
    gc.assert_refresh_sent();
    reload(&mut cs2, &gc);

    // the same among several
    let objects = CMsgSoMultipleObjects {
        objects_modified: vec![
            SingleObject { type_id: Some(ITEM_TYPE), object_data: Some(items::object(&items::item(2))) },
            SingleObject { type_id: Some(ITEM_TYPE), object_data: None },
        ],
        ..Default::default()
    };
    gc.send(SO_UPDATE_MULTIPLE, &objects);
    assert_eq!(drain(&mut cs2), ["unavailable"]);
    gc.assert_refresh_sent();
    reload(&mut cs2, &gc);

    // a removal of an item it can't read
    let destroyed =
        CMsgSoSingleObject { object_data: Some(vec![0x0a]), ..object(ITEM_TYPE, &items::item(1), Some(account)) };
    gc.send(SO_DESTROY, &destroyed);
    assert_eq!(drain(&mut cs2), ["unavailable"]);
    gc.assert_refresh_sent();
    reload(&mut cs2, &gc);

    // a shared-object message it can't read, whoever's it is
    for msg_type in [SO_UPDATE, SO_CACHE_SUBSCRIBED] {
        gc.push(GcMessage::new(
            APP_ID,
            GcPacket { body: Bytes::from_static(b"\x0a"), ..GcPacket::proto(msg_type, &()) },
        ));
        assert_eq!(drain(&mut cs2), ["unavailable"]);
        gc.assert_refresh_sent();
        reload(&mut cs2, &gc);
    }
    assert!(gc.try_to_gc().is_none());
}

#[tokio::test(start_paused = true)]
async fn the_inventory_reads_what_came_first() {
    let (mut cs2, gc) = session().await;
    gc.send(SO_CREATE, &object(ITEM_TYPE, &items::item(5), Some(gc.account)));
    // no event read: `inventory` reads the message itself
    assert_eq!(current_ids(&mut cs2), [5]);
    assert_eq!(drain(&mut cs2), ["acquired 5"]);
}

#[tokio::test(start_paused = true)]
async fn asks_the_game_coordinator_for_the_inventory_again() {
    let (mut cs2, mut gc) = session().await;
    gc.send(SO_CACHE_SUBSCRIPTION_CHECK, &check(gc.account, 99));
    assert!(cs2.inventory().unwrap().is_none());
    gc.assert_refresh_sent();
}

// `connect` on a loopback client (a real `SteamClient` on channels): what it sends Steam, and how the game
// coordinator's messages reach the session.

mod connect {
    use steamwire::client::testing::Loopback;
    use steamwire::Packet;

    use super::*;

    /// Reads what the client sends Steam up to its first message to a game coordinator, which must be a hello to
    /// CS2's. Returns the games played it sent before.
    async fn played_then_hello(steam: &mut Loopback) -> Vec<Vec<u64>> {
        let mut played = Vec::new();
        let to_gc = loop {
            let packet: Packet = steam.sent().await;
            if packet.emsg == EMsg::ClientGamesPlayed {
                let body: CMsgClientGamesPlayed = packet.decode().unwrap();
                played.push(body.games_played.iter().map(|game| game.game_id()).collect());
            } else if packet.emsg == EMsg::ClientToGC {
                break packet.decode::<steamwire::protos::steam::CMsgGcClient>().unwrap();
            }
        };
        let hello = GcPacket::parse(to_gc.msgtype(), Bytes::from(to_gc.payload.unwrap_or_default())).unwrap();
        assert_eq!((to_gc.appid, hello.msg_type), (Some(APP_ID), CLIENT_HELLO));
        assert_eq!(hello.decode::<CMsgClientHello>().unwrap(), super::super::hello());
        played
    }

    fn welcome(steam: &Loopback, account: SteamId) {
        steam.receive_gc(APP_ID, &GcPacket::proto(CLIENT_WELCOME, &welcome_with(account, &[])));
    }

    #[tokio::test(start_paused = true)]
    async fn needs_a_logged_on_account() {
        let steam = Loopback::new();
        assert!(matches!(Cs2::connect(&steam.client).await, Err(Error::NotLoggedOn)));
    }

    #[tokio::test(start_paused = true)]
    async fn plays_cs2_then_greets_until_welcomed() {
        let account = SteamId::from_u64(OWNER);
        let mut steam = Loopback::logged_on(account);
        let client = steam.client.clone();
        let steam_side = async {
            // CS2 alone is played before any hello
            assert_eq!(played_then_hello(&mut steam).await, [vec![u64::from(APP_ID)]]);
            welcome(&steam, account);
        };
        let (cs2, ()) = soon(async { tokio::join!(Cs2::connect(&client), steam_side) }).await;
        let mut cs2 = cs2.unwrap();
        assert!(cs2.has_session());
        assert_eq!(current_ids(&mut cs2), Vec::<u64>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn a_welcome_before_the_first_hello_is_taken() {
        let account = SteamId::from_u64(OWNER);
        let mut steam = Loopback::logged_on(account);
        let client = steam.client.clone();
        let steam_side = async {
            // the game coordinator welcomes the account as soon as it plays CS2
            steam.sent_of(EMsg::ClientGamesPlayed).await;
            welcome(&steam, account);
        };
        let (cs2, ()) = soon(async { tokio::join!(Cs2::connect(&client), steam_side) }).await;
        let cs2 = cs2.unwrap();
        assert!(cs2.has_session());
        // welcomed, so no hello was sent at all, and while the session lives, CS2's messages (the welcome too) don't
        // go to the events
        steam.receive_gc(APP_ID, &GcPacket::plain(4004, vec![1]));
        steam.receive_gc(440, &GcPacket::plain(4004, vec![2]));
        let packet = steam.events.recv_msg(EMsg::ClientFromGC).await.unwrap();
        assert_eq!(GcMessage::from_packet(&packet).unwrap().app_id, 440);
        assert!(!std::iter::from_fn(|| steam.try_sent()).any(|packet| packet.emsg == EMsg::ClientToGC));
    }

    #[tokio::test(start_paused = true)]
    async fn takes_the_messages_of_an_earlier_subscription() {
        let account = SteamId::from_u64(OWNER);
        let mut steam = Loopback::logged_on(account);
        let mut earlier = steam.client.subscribe_gc(APP_ID);
        let client = steam.client.clone();
        let steam_side = async {
            played_then_hello(&mut steam).await;
            welcome(&steam, account);
        };
        let (cs2, ()) = soon(async { tokio::join!(Cs2::connect(&client), steam_side) }).await;
        assert!(cs2.unwrap().has_session());
        assert!(earlier.recv().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn ends_when_the_client_disconnects_while_greeting() {
        let mut steam = Loopback::logged_on(SteamId::from_u64(OWNER));
        let client = steam.client.clone();
        let steam_side = async {
            played_then_hello(&mut steam).await;
            steam.close();
        };
        let (cs2, ()) = soon(async { tokio::join!(Cs2::connect(&client), steam_side) }).await;
        assert!(matches!(cs2, Err(Error::Closed)));
    }

    #[tokio::test(start_paused = true)]
    async fn fails_when_the_game_coordinator_refuses() {
        let mut steam = Loopback::logged_on(SteamId::from_u64(OWNER));
        let client = steam.client.clone();
        let steam_side = async {
            played_then_hello(&mut steam).await;
            steam.receive_gc(APP_ID, &GcPacket::proto(LOGON_FATAL_ERROR, &fatal(5, "banned")));
        };
        let (cs2, ()) = soon(async { tokio::join!(Cs2::connect(&client), steam_side) }).await;
        assert!(matches!(cs2, Err(Error::Refused { code: 5, .. })));
    }
}
