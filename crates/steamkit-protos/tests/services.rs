//! The generated service clients route calls and notifications by their
//! `Service.Method#1` names, as SteamKit2's generated service classes do.

use std::future::{ready, Future};
use std::pin::pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use prost::Message;
use steamkit_protos::steam::services::{FriendMessagesClientNotification, Player, PlayerClient};
use steamkit_protos::steam::{
    CFriendMessagesIncomingMessageNotification, CPlayerGetGameBadgeLevelsRequest, CPlayerGetGameBadgeLevelsResponse,
    CPlayerLastPlayedTimesNotification,
};
use steamkit_protos::ServiceTransport;

/// Records each call as `(kind, method, encoded request)` and answers calls
/// with a default response.
#[derive(Default)]
struct Recorder(Mutex<Vec<(&'static str, &'static str, Vec<u8>)>>);

impl ServiceTransport for Recorder {
    type Error = ();

    fn call<Req, Resp>(&self, method: &'static str, request: Req) -> impl Future<Output = Result<Resp, ()>> + Send
    where
        Req: Message + Send + 'static,
        Resp: Message + Default + Send + 'static,
    {
        self.0.lock().unwrap().push(("call", method, request.encode_to_vec()));
        ready(Ok(Resp::default()))
    }

    fn notify<Req>(&self, method: &'static str, request: Req) -> Result<(), ()>
    where
        Req: Message + Send + 'static,
    {
        self.0.lock().unwrap().push(("notify", method, request.encode_to_vec()));
        Ok(())
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the recorder answers immediately"),
    }
}

#[test]
fn call_and_notification_routing() {
    let recorder = Recorder::default();
    let player = Player::new(&recorder);
    assert_eq!(Player::<Recorder>::NAME, "Player");

    let request = CPlayerGetGameBadgeLevelsRequest { appid: Some(440) };
    let response: CPlayerGetGameBadgeLevelsResponse = block_on(player.get_game_badge_levels(request)).unwrap();
    assert_eq!(response, CPlayerGetGameBadgeLevelsResponse::default());

    // Returns NoResponse, so it's a notification.
    let last_played = CPlayerLastPlayedTimesNotification::default();
    PlayerClient::new(&recorder).notify_last_played_times(last_played.clone()).unwrap();

    let calls = recorder.0.into_inner().unwrap();
    assert_eq!(
        calls,
        [
            ("call", "Player.GetGameBadgeLevels#1", request.encode_to_vec()),
            ("notify", "PlayerClient.NotifyLastPlayedTimes#1", last_played.encode_to_vec()),
        ]
    );
}

#[test]
fn notification_decode() {
    let message = CFriendMessagesIncomingMessageNotification {
        steamid_friend: Some(76561197960287930),
        message: Some("hello".into()),
        ..Default::default()
    };
    let body = message.encode_to_vec();

    let decoded = FriendMessagesClientNotification::decode("FriendMessagesClient.IncomingMessage#1", &body);
    assert_eq!(decoded, Some(Ok(FriendMessagesClientNotification::IncomingMessage(message))));

    // Not a notification of this service.
    assert_eq!(FriendMessagesClientNotification::decode("FriendMessages.AckMessage#1", &body), None);
    // A malformed body.
    assert!(matches!(
        FriendMessagesClientNotification::decode("FriendMessagesClient.IncomingMessage#1", &[0xff]),
        Some(Err(_))
    ));
}
