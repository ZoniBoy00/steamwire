use tokio::sync::mpsc;

use crate::enums::EMsg;
use crate::messages::Packet;

/// Something that happened on the connection.
#[derive(Debug, Clone)]
pub enum Event {
    /// A message that no job or waiter consumed.
    Message(Packet),
    /// The connection is gone. No more events follow.
    Disconnected {
        /// Whether [`SteamClient::disconnect`](crate::SteamClient::disconnect)
        /// or [`SteamClient::log_off`](crate::SteamClient::log_off) was called.
        user_initiated: bool,
    },
}

/// The receiving end of a client's events.
#[derive(Debug)]
pub struct Events {
    pub(super) rx: mpsc::UnboundedReceiver<Event>,
}

impl Events {
    /// The next event, or `None` once the client is gone.
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    /// The next message, skipping nothing; `None` once disconnected.
    pub async fn recv_packet(&mut self) -> Option<Packet> {
        match self.rx.recv().await? {
            Event::Message(packet) => Some(packet),
            Event::Disconnected { .. } => None,
        }
    }

    /// The next message of the given type; other messages are dropped.
    /// `None` when the connection ends first.
    pub async fn recv_msg(&mut self, emsg: EMsg) -> Option<Packet> {
        while let Some(event) = self.rx.recv().await {
            match event {
                Event::Message(packet) if packet.emsg == emsg => return Some(packet),
                Event::Message(_) => {}
                Event::Disconnected { .. } => return None,
            }
        }
        None
    }
}
