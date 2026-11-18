//! Game coordinator messages (SteamKit2's `SteamGameCoordinator`). GC messages
//! travel inside `ClientToGC` / `ClientFromGC`; see [`GcPacket`] for framing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use steamwire_protos::steam::CMsgGcClient;
use tokio::sync::{mpsc, oneshot};

use crate::client::SteamClient;
use crate::enums::EMsg;
use crate::error::{Error, Result};
use crate::messages::gc::GcPacket;
use crate::messages::{Packet, ProtoMsg};

/// A message from a game coordinator (`ClientFromGC`).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GcMessage {
    /// The app whose GC sent it.
    pub app_id: u32,
    /// The message.
    pub packet: GcPacket,
}

impl GcMessage {
    /// A message from `app_id`'s GC, such as a test's stand-in for one.
    pub fn new(app_id: u32, packet: GcPacket) -> Self {
        Self { app_id, packet }
    }

    /// Decodes a `ClientFromGC` packet.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        if packet.emsg != EMsg::ClientFromGC {
            return None;
        }
        let msg: CMsgGcClient = packet.decode().ok()?;
        let app_id = msg.appid();
        let gc = GcPacket::parse(msg.msgtype(), Bytes::from(msg.payload.unwrap_or_default())).ok()?;
        Some(Self { app_id, packet: gc })
    }

    /// The GC message type, without the protobuf flag.
    pub fn msg_type(&self) -> u32 {
        self.packet.msg_type
    }

    /// Whether the message has a protobuf header.
    pub fn is_proto(&self) -> bool {
        self.packet.is_proto()
    }
}

/// GC requests waiting for a reply, by source job, and the apps whose GC
/// messages go to a [`GcSubscription`].
#[derive(Debug, Default)]
pub(crate) struct GcJobs {
    pending: Mutex<HashMap<u64, oneshot::Sender<GcMessage>>>,
    routes: Mutex<HashMap<u32, mpsc::UnboundedSender<GcMessage>>>,
}

impl GcJobs {
    /// Hands a GC reply to its waiting request, or a GC message to its app's
    /// subscription. Returns whether it was taken.
    pub(crate) fn observe(&self, packet: &Packet) -> bool {
        if packet.emsg != EMsg::ClientFromGC {
            return false;
        }
        let has_pending = !self.pending.lock().unwrap().is_empty();
        let has_routes = !self.routes.lock().unwrap().is_empty();
        if !has_pending && !has_routes {
            return false;
        }
        let Some(msg) = GcMessage::from_packet(packet) else {
            return false;
        };
        let target = msg.packet.target_job();
        if has_pending && !target.is_none() {
            if let Some(tx) = self.pending.lock().unwrap().remove(&target.0) {
                if tx.send(msg.clone()).is_ok() {
                    return true;
                }
            }
        }
        let app_id = msg.app_id;
        let mut routes = self.routes.lock().unwrap();
        let Some(route) = routes.get(&app_id) else {
            return false;
        };
        if route.send(msg).is_ok() {
            return true;
        }
        // The subscription is gone: back to the events.
        routes.remove(&app_id);
        false
    }

    /// Ends the subscriptions (on disconnect): their `recv` gives `None`.
    pub(crate) fn clear_routes(&self) {
        self.routes.lock().unwrap().clear();
    }
}

/// The messages of one app's game coordinator, from
/// [`SteamClient::subscribe_gc`]: replies to [`SteamClient::call_gc`] still
/// go to their call, and everything else from that GC comes here instead of
/// [`Events`](crate::Events).
#[derive(Debug)]
pub struct GcSubscription {
    app_id: u32,
    rx: mpsc::UnboundedReceiver<GcMessage>,
}

impl GcSubscription {
    /// The app whose GC messages these are.
    pub fn app_id(&self) -> u32 {
        self.app_id
    }

    /// The next message; `None` once the client disconnected, or another
    /// subscription for the same app replaced this one.
    pub async fn recv(&mut self) -> Option<GcMessage> {
        self.rx.recv().await
    }

    /// The next message if one is waiting, without waiting:
    /// [`TryRecvError::Empty`](mpsc::error::TryRecvError::Empty) if none is,
    /// `Disconnected` when [`recv`](Self::recv) would give `None`.
    pub fn try_recv(&mut self) -> std::result::Result<GcMessage, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

impl SteamClient {
    /// Sends a message to an app's game coordinator.
    pub fn send_gc(&self, app_id: u32, packet: &GcPacket) -> Result<()> {
        let mut msg = ProtoMsg::new(
            EMsg::ClientToGC,
            CMsgGcClient {
                appid: Some(app_id),
                msgtype: Some(packet.wire_type()),
                payload: Some(packet.encode()),
                ..Default::default()
            },
        );
        msg.header.routing_appid = Some(app_id);
        self.send(msg)
    }

    /// Sends the messages of `app_id`'s game coordinator to the returned
    /// subscription instead of [`Events`](crate::Events), until it is
    /// dropped (they go to the events again) or the client disconnects. A
    /// second subscription for the same app replaces the first, which then
    /// ends. Replies to [`call_gc`](Self::call_gc) still go to their call.
    pub fn subscribe_gc(&self, app_id: u32) -> GcSubscription {
        let (tx, rx) = mpsc::unbounded_channel();
        self.handler_state().gc_jobs.routes.lock().unwrap().insert(app_id, tx);
        GcSubscription { app_id, rx }
    }

    /// Sends a GC message as a job and waits for the reply that targets it.
    /// Fails with [`Error::Timeout`] after `timeout`.
    pub async fn call_gc(&self, app_id: u32, mut packet: GcPacket, timeout: Duration) -> Result<GcMessage> {
        let job = self.new_job_id();
        packet.set_source_job(job);
        let (tx, rx) = oneshot::channel();
        let jobs = &self.handler_state().gc_jobs;
        jobs.pending.lock().unwrap().insert(job.0, tx);
        let result = match self.send_gc(app_id, &packet) {
            Ok(()) => match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(msg)) => Ok(msg),
                Ok(Err(_)) => Err(Error::NotConnected),
                Err(_) => Err(Error::Timeout),
            },
            Err(e) => Err(e),
        };
        jobs.pending.lock().unwrap().remove(&job.0);
        result
    }
}
