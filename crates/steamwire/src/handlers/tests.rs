//! Handler behaviour against a client without a network: server messages are
//! dispatched directly ([`Loopback`]) and requests are read back.

use std::io::Write as _;

use crate::protos::steam::{
    c_msg_client_friends_list, c_msg_client_mms_lobby_data, c_msg_client_persona_state,
    CAuthenticationBeginAuthSessionViaCredentialsResponse, CAuthenticationGetPasswordRsaPublicKeyResponse,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse, CMsgClientChangeStatus, CMsgClientFriendsList,
    CMsgClientMmsLobbyData, CMsgClientPersonaState, CMsgMulti,
};
use prost::Message;

use super::auth::{AuthSessionDetails, GuardCodeType};
use crate::client::testing::Loopback;
use crate::enums::{EClientPersonaStateFlag, EFriendRelationship, ELobbyType, EMsg, EPersonaState, EResult};
use crate::messages::ProtoMsg;
use crate::{Error, Event, SteamId};

const ME: SteamId = SteamId::from_u64(76561198000000001);
const FRIEND: SteamId = SteamId::from_u64(76561198000000002);
const LOBBY: SteamId = SteamId::from_u64(109775244945468851);

fn friends_list(incremental: bool, friends: &[(SteamId, EFriendRelationship)]) -> ProtoMsg<CMsgClientFriendsList> {
    ProtoMsg::new(
        EMsg::ClientFriendsList,
        CMsgClientFriendsList {
            bincremental: Some(incremental),
            friends: friends
                .iter()
                .map(|(id, rel)| c_msg_client_friends_list::Friend {
                    ulfriendid: Some(id.as_u64()),
                    efriendrelationship: Some(rel.0 as u32),
                })
                .collect(),
            ..Default::default()
        },
    )
}

fn persona_state(flags: EClientPersonaStateFlag, id: SteamId, name: &str) -> ProtoMsg<CMsgClientPersonaState> {
    ProtoMsg::new(
        EMsg::ClientPersonaState,
        CMsgClientPersonaState {
            status_flags: Some(flags.0 as u32),
            friends: vec![c_msg_client_persona_state::Friend {
                friendid: Some(id.as_u64()),
                player_name: Some(name.into()),
                persona_state: Some(EPersonaState::Online.0 as u32),
                ..Default::default()
            }],
        },
    )
}

#[tokio::test]
async fn logon_response_sets_the_session() {
    let mut steam = Loopback::new();
    assert!(!steam.client.is_logged_on());
    steam = {
        drop(steam);
        Loopback::logged_on(ME)
    };
    assert!(steam.client.is_logged_on());
    assert_eq!(steam.client.steam_id(), Some(ME));
    assert_eq!(steam.client.session_id(), Some(1234));
    assert_eq!(steam.client.cell_id(), Some(7));
}

#[tokio::test]
async fn friends_list_and_persona_names() {
    let steam = Loopback::logged_on(ME);
    steam.receive_proto(friends_list(false, &[(FRIEND, EFriendRelationship::Friend)]));
    assert_eq!(steam.client.friends(), vec![FRIEND]);
    assert_eq!(steam.client.friend(FRIEND).unwrap().relationship, EFriendRelationship::Friend);

    // As in SteamKit2, the name is taken only when the PlayerName flag is set.
    steam.receive_proto(persona_state(EClientPersonaStateFlag::Presence, FRIEND, "ignored"));
    assert_eq!(steam.client.friend(FRIEND).unwrap().name, None);
    assert_eq!(steam.client.friend(FRIEND).unwrap().persona_state, EPersonaState::Online);
    steam.receive_proto(persona_state(EClientPersonaStateFlag::PlayerName, FRIEND, "Friend"));
    assert_eq!(steam.client.friend(FRIEND).unwrap().name.as_deref(), Some("Friend"));

    // Our own persona state names us.
    steam.receive_proto(persona_state(EClientPersonaStateFlag::PlayerName, ME, "Me"));
    assert_eq!(steam.client.persona_name().as_deref(), Some("Me"));
    assert_eq!(steam.client.persona_state(), EPersonaState::Offline);
    steam.receive_proto(persona_state(EClientPersonaStateFlag::Presence, ME, "Me"));
    assert_eq!(steam.client.persona_state(), EPersonaState::Online);
    assert!(steam.client.friend(ME).is_none(), "we aren't our own friend");

    // An incremental list changes only the friends in it.
    let other = SteamId::from_u64(76561198000000003);
    steam.receive_proto(friends_list(true, &[(other, EFriendRelationship::RequestRecipient)]));
    assert_eq!(steam.client.friend(FRIEND).unwrap().relationship, EFriendRelationship::Friend);
    assert_eq!(steam.client.friend(other).unwrap().relationship, EFriendRelationship::RequestRecipient);
    steam.receive_proto(friends_list(true, &[(FRIEND, EFriendRelationship::None)]));
    assert!(!steam.client.friends().contains(&FRIEND));
}

#[tokio::test]
async fn persona_name_change_keeps_the_state() {
    let mut steam = Loopback::logged_on(ME);
    steam.client.set_persona_state(EPersonaState::Away).unwrap();
    let sent = steam.sent_of(EMsg::ClientChangeStatus).await.decode::<CMsgClientChangeStatus>().unwrap();
    assert_eq!(sent.persona_state, Some(EPersonaState::Away.0 as u32));

    steam.client.set_persona_name("New name").unwrap();
    let packet = steam.sent_of(EMsg::ClientChangeStatus).await;
    assert_eq!(packet.steam_id(), Some(ME), "the session is filled in");
    let sent = packet.decode::<CMsgClientChangeStatus>().unwrap();
    assert_eq!(sent.player_name.as_deref(), Some("New name"));
    assert_eq!(sent.persona_state, Some(EPersonaState::Away.0 as u32));
}

#[tokio::test]
async fn lobby_data_is_cached() {
    let steam = Loopback::logged_on(ME);
    steam.receive_proto(ProtoMsg::new(
        EMsg::ClientMMSLobbyData,
        CMsgClientMmsLobbyData {
            app_id: Some(480),
            steam_id_lobby: Some(LOBBY.as_u64()),
            num_members: Some(2),
            max_members: Some(4),
            lobby_type: Some(ELobbyType::FriendsOnly.0),
            steam_id_owner: Some(FRIEND.as_u64()),
            members: [ME, FRIEND]
                .iter()
                .map(|id| c_msg_client_mms_lobby_data::Member {
                    steam_id: Some(id.as_u64()),
                    persona_name: Some("someone".into()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        },
    ));
    let lobby = steam.client.lobby(480, LOBBY).expect("cached");
    assert_eq!(lobby.owner, Some(FRIEND));
    assert_eq!(lobby.lobby_type, ELobbyType::FriendsOnly);
    assert_eq!((lobby.num_members, lobby.max_members), (2, 4));
    assert_eq!(lobby.members.len(), 2);
    assert!(steam.client.lobby(440, LOBBY).is_none(), "lobbies are per app");
}

#[tokio::test]
async fn jobs_get_their_reply_and_failures() {
    let mut steam = Loopback::logged_on(ME);
    let client = steam.client.clone();
    let call = tokio::spawn(async move { client.call_service::<_, CMsgClientChangeStatus>("Test.Method#1", ()).await });
    let request = steam.sent().await;
    assert_eq!(request.proto_header().unwrap().target_job_name.as_deref(), Some("Test.Method#1"));
    let reply = CMsgClientChangeStatus { player_name: Some("answer".into()), ..Default::default() };
    steam.reply(&request, EMsg::ServiceMethodResponse, EResult::OK, reply.clone());
    assert_eq!(call.await.unwrap().unwrap(), reply);

    // An error result in the header becomes Error::Steam.
    let client = steam.client.clone();
    let call = tokio::spawn(async move { client.call_service::<_, CMsgClientChangeStatus>("Test.Method#1", ()).await });
    let request = steam.sent().await;
    steam.reply(&request, EMsg::ServiceMethodResponse, EResult::AccessDenied, ());
    assert_eq!(call.await.unwrap().unwrap_err().result(), Some(EResult::AccessDenied));

    // DestJobFailed fails the job.
    let client = steam.client.clone();
    let call = tokio::spawn(async move { client.call_service::<_, CMsgClientChangeStatus>("Test.Method#1", ()).await });
    let request = steam.sent().await;
    steam.reply(&request, EMsg::DestJobFailed, EResult::OK, ());
    assert!(matches!(call.await.unwrap(), Err(Error::JobFailed)));
}

#[tokio::test]
async fn multi_messages_are_unpacked() {
    let mut steam = Loopback::logged_on(ME);
    let mut payload = Vec::new();
    for msg in [
        friends_list(false, &[(FRIEND, EFriendRelationship::Friend)]).encode(),
        ProtoMsg::new(EMsg::ClientServersAvailable, ()).encode(),
    ] {
        payload.extend_from_slice(&(msg.len() as u32).to_le_bytes());
        payload.extend_from_slice(&msg);
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&payload).unwrap();
    let multi = CMsgMulti { size_unzipped: Some(payload.len() as u32), message_body: Some(gz.finish().unwrap()) };
    steam.receive_proto(ProtoMsg::new(EMsg::Multi, multi));

    assert_eq!(steam.client.friends(), vec![FRIEND]);
    // A message no handler takes arrives as an event.
    loop {
        match steam.events.recv().await {
            Some(Event::Message(packet)) if packet.emsg == EMsg::ClientServersAvailable => break,
            Some(_) => continue,
            None => panic!("events ended"),
        }
    }
}

/// Regression test for a bug found live: Steam answers a code with
/// DuplicateRequest once the login was approved in the app, which SteamKit2
/// treats as success.
#[tokio::test]
async fn guard_code_duplicate_request_is_success() {
    let mut steam = Loopback::new();
    let client = steam.client.clone();
    let begin = tokio::spawn(async move {
        client.begin_auth_session_via_credentials("user", "password", &AuthSessionDetails::default()).await
    });

    // Any RSA key will do: Steam's universe key.
    let der = crate::net::encryption::universe_public_key(crate::enums::EUniverse::Public).unwrap();
    let key = steam.sent().await;
    steam.reply(
        &key,
        EMsg::ServiceMethodResponse,
        EResult::OK,
        CAuthenticationGetPasswordRsaPublicKeyResponse {
            publickey_mod: Some(hex::encode(&der[29..157])),
            publickey_exp: Some("11".into()),
            timestamp: Some(1),
        },
    );
    let start = steam.sent().await;
    steam.reply(
        &start,
        EMsg::ServiceMethodResponse,
        EResult::OK,
        CAuthenticationBeginAuthSessionViaCredentialsResponse {
            client_id: Some(5),
            request_id: Some(vec![1, 2, 3]),
            interval: Some(5.0),
            steamid: Some(ME.as_u64()),
            ..Default::default()
        },
    );
    let session = begin.await.unwrap().unwrap();

    for (result, expected) in
        [(EResult::DuplicateRequest, None), (EResult::InvalidLoginAuthCode, Some(EResult::InvalidLoginAuthCode))]
    {
        let server = async {
            let request = steam.sent().await;
            let body = CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest::decode(request.body.clone()).unwrap();
            assert_eq!((body.client_id, body.code.as_deref()), (Some(5), Some("ABCDE")));
            let reply = CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse::default();
            steam.reply(&request, EMsg::ServiceMethodResponse, result, reply);
        };
        let (submitted, ()) = tokio::join!(session.submit_code("ABCDE", GuardCodeType::Device), server);
        assert_eq!(submitted.err().and_then(|e| e.result()), expected);
    }
}

#[tokio::test]
async fn gc_subscriptions_take_their_apps_messages() {
    use crate::messages::gc::GcPacket;

    let mut steam = Loopback::logged_on(ME);
    let mut cs2 = steam.client.subscribe_gc(730);
    assert_eq!(cs2.app_id(), 730);

    // The subscribed app's messages go to the subscription, others to the events.
    steam.receive_gc(730, &GcPacket::plain(4004, vec![1]));
    steam.receive_gc(440, &GcPacket::plain(4004, vec![2]));
    let msg = cs2.recv().await.unwrap();
    assert_eq!((msg.app_id, msg.msg_type(), &msg.packet.body[..]), (730, 4004, &[1][..]));
    assert!(matches!(cs2.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
    let packet = steam.events.recv_msg(EMsg::ClientFromGC).await.unwrap();
    assert_eq!(super::game_coordinator::GcMessage::from_packet(&packet).unwrap().app_id, 440);

    // A reply to a call still goes to the call.
    let client = steam.client.clone();
    let call = client.call_gc(730, GcPacket::plain(9000, vec![]), std::time::Duration::from_secs(5));
    let server = async {
        let (app_id, request) = steam.sent_gc().await;
        assert_eq!(app_id, 730);
        let mut reply = GcPacket::plain(9001, vec![3]);
        reply.set_target_job(request.source_job());
        steam.receive_gc(730, &reply);
    };
    let (reply, ()) = tokio::join!(call, server);
    assert_eq!(reply.unwrap().msg_type(), 9001);
    assert!(matches!(cs2.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));

    // A second subscription replaces the first; a dropped one sends the
    // messages back to the events.
    let second = steam.client.subscribe_gc(730);
    assert!(cs2.recv().await.is_none());
    drop(second);
    steam.receive_gc(730, &GcPacket::plain(4004, vec![4]));
    let packet = steam.events.recv_msg(EMsg::ClientFromGC).await.unwrap();
    assert_eq!(super::game_coordinator::GcMessage::from_packet(&packet).unwrap().packet.body[..], [4]);

    // Disconnecting ends the subscriptions.
    let mut third = steam.client.subscribe_gc(730);
    steam.close();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), third.recv()).await;
    assert!(ended.unwrap().is_none());
}
