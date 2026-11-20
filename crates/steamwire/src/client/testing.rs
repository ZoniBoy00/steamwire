//! A client wired to channels instead of a socket, for tests: messages from
//! "Steam" are dispatched directly and what the client sends is read back.

use bytes::Bytes;
use prost::Message;
use tokio::sync::mpsc;

use super::{Config, Events, SteamClient};
use crate::enums::{EMsg, EResult};
use crate::messages::gc::GcPacket;
use crate::messages::{Packet, ProtoMsg};
use crate::net::{CmServer, Connection, Outgoing};
use crate::types::SteamId;

/// A [`SteamClient`] whose connection is a pair of channels.
pub struct Loopback {
    /// The client.
    pub client: SteamClient,
    /// Its events.
    pub events: Events,
    sent: mpsc::UnboundedReceiver<Outgoing>,
    // Kept so the reader task doesn't see the connection close.
    incoming: Option<mpsc::UnboundedSender<Bytes>>,
}

impl std::fmt::Debug for Loopback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loopback").field("client", &self.client).finish_non_exhaustive()
    }
}

impl Loopback {
    /// A connected client; needs a tokio runtime. The `ClientHello` it sends
    /// is skipped.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (outgoing, sent) = mpsc::unbounded_channel();
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let conn = Connection { outgoing, incoming, local_ip: None };
        let (client, events) = SteamClient::start(conn, CmServer::tcp("127.0.0.1:27017"), &Config::default());
        let mut loopback = Self { client, events, sent, incoming: Some(incoming_tx) };
        assert_eq!(loopback.try_sent().map(|p| p.emsg), Some(EMsg::ClientHello));
        loopback
    }

    /// A client logged on as `steam_id`.
    pub fn logged_on(steam_id: SteamId) -> Self {
        let loopback = Self::new();
        let mut response = ProtoMsg::new(
            EMsg::ClientLogOnResponse,
            crate::protos::steam::CMsgClientLogonResponse {
                eresult: Some(EResult::OK.0),
                heartbeat_seconds: Some(9),
                cell_id: Some(7),
                ..Default::default()
            },
        );
        response.header.steamid = Some(steam_id.as_u64());
        response.header.client_sessionid = Some(1234);
        loopback.receive_proto(response);
        loopback
    }

    /// Closes the connection from the server's side.
    pub fn close(&mut self) {
        self.incoming = None;
    }

    /// Dispatches raw bytes as if they came from the server.
    pub fn receive(&self, data: Vec<u8>) {
        self.client.dispatch_raw(Bytes::from(data));
    }

    /// Dispatches `msg` as if it came from the server.
    pub fn receive_proto<M: Message>(&self, msg: ProtoMsg<M>) {
        self.receive(msg.encode());
    }

    /// Dispatches `packet` as if `app_id`'s game coordinator sent it (in a
    /// `ClientFromGC`).
    pub fn receive_gc(&self, app_id: u32, packet: &GcPacket) {
        self.receive_proto(ProtoMsg::new(
            EMsg::ClientFromGC,
            crate::protos::steam::CMsgGcClient {
                appid: Some(app_id),
                msgtype: Some(packet.wire_type()),
                payload: Some(packet.encode()),
                ..Default::default()
            },
        ));
    }

    /// Answers `request` (a job) with `body`, with `result` in the header.
    pub fn reply<M: Message>(&self, request: &Packet, emsg: EMsg, result: EResult, body: M) {
        let mut reply = ProtoMsg::new(emsg, body);
        reply.header.jobid_target = Some(request.source_job().0);
        reply.header.eresult = Some(result.0);
        self.receive_proto(reply);
    }

    /// The next message the client sent, if any.
    pub fn try_sent(&mut self) -> Option<Packet> {
        loop {
            match self.sent.try_recv().ok()? {
                Outgoing::Data(data) => {
                    return Some(Packet::parse(Bytes::from(data)).expect("client sent a valid packet"))
                }
                Outgoing::Close => continue,
            }
        }
    }

    /// Waits for the next message the client sends.
    pub async fn sent(&mut self) -> Packet {
        let wait = async {
            loop {
                match self.sent.recv().await.expect("client is alive") {
                    Outgoing::Data(data) => {
                        return Packet::parse(Bytes::from(data)).expect("client sent a valid packet")
                    }
                    Outgoing::Close => continue,
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), wait).await.expect("the client sent nothing")
    }

    /// Waits for the next message of type `emsg`, skipping others.
    pub async fn sent_of(&mut self, emsg: EMsg) -> Packet {
        loop {
            let packet = self.sent().await;
            if packet.emsg == emsg {
                return packet;
            }
        }
    }

    /// Waits for the next message to a game coordinator (`ClientToGC`),
    /// skipping others. Returns the app and the message.
    pub async fn sent_gc(&mut self) -> (u32, GcPacket) {
        let packet = self.sent_of(EMsg::ClientToGC).await;
        let msg: crate::protos::steam::CMsgGcClient = packet.decode().expect("a ClientToGC");
        let app_id = msg.appid();
        let gc = GcPacket::parse(msg.msgtype(), Bytes::from(msg.payload.unwrap_or_default())).expect("a GC packet");
        (app_id, gc)
    }
}
