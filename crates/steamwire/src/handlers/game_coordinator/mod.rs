//! Game coordinator messages (SteamKit2's `SteamGameCoordinator`). GC messages
//! travel inside `ClientToGC` / `ClientFromGC`; see [`GcPacket`] for framing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use steamwire_protos::steam::CMsgGcClient;
use tokio::sync::oneshot;

use crate::client::SteamClient;
use crate::enums::EMsg;
use crate::error::{Error, Result};
use crate::messages::gc::GcPacket;
use crate::messages::{Packet, ProtoMsg};

/// A message from a game coordinator (`ClientFromGC`).
#[derive(Clone, Debug)]
pub struct GcMessage {
    /// The app whose GC sent it.
    pub app_id: u32,
    /// The message.
    pub packet: GcPacket,
}

impl GcMessage {
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

/// GC requests waiting for a reply, by source job.
#[derive(Debug, Default)]
pub(crate) struct GcJobs {
    pending: Mutex<HashMap<u64, oneshot::Sender<GcMessage>>>,
}

impl GcJobs {
    /// Hands a GC reply to its waiting request. Returns whether it was taken.
    pub(crate) fn observe(&self, packet: &Packet) -> bool {
        if packet.emsg != EMsg::ClientFromGC {
            return false;
        }
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            return false;
        }
        let Some(msg) = GcMessage::from_packet(packet) else {
            return false;
        };
        let target = msg.packet.target_job();
        if target.is_none() {
            return false;
        }
        match pending.remove(&target.0) {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
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
