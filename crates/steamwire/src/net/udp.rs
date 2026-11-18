//! Steam's reliable UDP protocol (SteamKit2's `UdpConnection` and
//! `UdpPacket`): a challenge/connect handshake, then sequenced packets with
//! acks and resends. Channel encryption is added by `net/envelope.rs`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Connection, Outgoing};
use crate::enums::EUdpPacketType;
use crate::error::{Error, Result};
use crate::messages::generated::{ChallengeData, ConnectData, UdpHeader};
use crate::messages::{Reader, SteamStruct};

/// The largest payload of one packet.
pub const MAX_PAYLOAD: usize = 0x4DC;
/// The size of [`UdpHeader`] on the wire.
pub const HEADER_SIZE: usize = 36;

const RESEND_DELAY: Duration = Duration::from_secs(3);
const TIMEOUT_DELAY: Duration = Duration::from_secs(60);
const RESEND_COUNT: usize = 3;
const AHEAD_COUNT: u32 = 5;
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Our connection ID; advanced by 256 per connection, like Steam does when
/// a socket is reused.
static SOURCE_CONN_ID: AtomicU32 = AtomicU32::new(512);

/// A UDP packet: header and up to [`MAX_PAYLOAD`] bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct UdpPacket {
    /// The header.
    pub header: UdpHeader,
    /// The payload.
    pub payload: Vec<u8>,
}

impl UdpPacket {
    /// An empty packet.
    pub fn new(packet_type: EUdpPacketType) -> Self {
        Self { header: UdpHeader { packet_type, ..UdpHeader::default() }, payload: Vec::new() }
    }

    /// A packet with a payload; fails if it's over [`MAX_PAYLOAD`].
    pub fn with_payload(packet_type: EUdpPacketType, payload: Vec<u8>) -> Result<Self> {
        let mut packet = Self::new(packet_type);
        packet.set_payload(payload)?;
        Ok(packet)
    }

    /// Sets the payload and the sizes in the header.
    pub fn set_payload(&mut self, payload: Vec<u8>) -> Result<()> {
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::Other(format!("UDP payload of {} bytes exceeds {MAX_PAYLOAD}", payload.len())));
        }
        self.header.payload_size = payload.len() as u16;
        self.header.msg_size = payload.len() as u32;
        self.payload = payload;
        Ok(())
    }

    /// Parses a datagram; `None` if it isn't a valid packet.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut r = Reader::new(data);
        let header = UdpHeader::decode(&mut r).ok()?;
        if header.magic != UdpHeader::MAGIC || header.payload_size as usize > MAX_PAYLOAD {
            return None;
        }
        let payload = r.bytes(header.payload_size as usize).ok()?.to_vec();
        Some(Self { header, payload })
    }

    /// Encodes the packet.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        self.header.encode(&mut out);
        out.extend_from_slice(&self.payload);
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    ChallengeReqSent,
    ConnectSent,
    Connected,
    Disconnecting,
    Disconnected,
}

struct Link {
    sock: UdpSocket,
    state: State,
    source_conn_id: u32,
    remote_conn_id: u32,
    out_seq: u32,
    out_seq_sent: u32,
    out_seq_acked: u32,
    in_seq: u32,
    in_seq_acked: u32,
    in_seq_handled: u32,
    out_packets: VecDeque<UdpPacket>,
    in_packets: HashMap<u32, UdpPacket>,
    timeout: Instant,
    next_resend: Instant,
    deliver: mpsc::UnboundedSender<Bytes>,
    connected: Option<oneshot::Sender<()>>,
}

impl Link {
    async fn send_packet(&mut self, mut packet: UdpPacket) {
        packet.header.source_conn_id = self.source_conn_id;
        packet.header.dest_conn_id = self.remote_conn_id;
        self.in_seq_acked = self.in_seq;
        packet.header.seq_ack = self.in_seq;
        tracing::trace!(kind = ?packet.header.packet_type, seq = packet.header.seq_this, ack = packet.header.seq_ack, "UDP send");
        if let Err(e) = self.sock.send(&packet.to_bytes()).await {
            tracing::debug!("UDP send failed: {e}");
            self.state = State::Disconnected;
            return;
        }
        // After being idle but fully acked, don't trip the resend check.
        if self.out_seq_sent == self.out_seq_acked {
            self.next_resend = Instant::now() + RESEND_DELAY;
        }
        if packet.header.seq_this > 0 {
            self.out_seq_sent = self.out_seq_sent.max(packet.header.seq_this);
        }
    }

    /// Queues packets forming one message.
    fn send_sequenced(&mut self, mut packets: Vec<UdpPacket>) {
        let start = self.out_seq;
        let count = packets.len() as u32;
        for packet in &mut packets {
            packet.header.seq_this = self.out_seq;
            packet.header.msg_start_seq = start;
            packet.header.packets_in_msg = count;
            self.out_seq += 1;
        }
        self.out_packets.extend(packets);
    }

    fn send_data(&mut self, data: &[u8]) {
        let packets = data
            .chunks(MAX_PAYLOAD)
            .map(|part| {
                let mut p = UdpPacket::with_payload(EUdpPacketType::Data, part.to_vec()).expect("chunk fits");
                p.header.msg_size = data.len() as u32;
                p
            })
            .collect::<Vec<_>>();
        let packets = if packets.is_empty() { vec![UdpPacket::new(EUdpPacketType::Data)] } else { packets };
        self.send_sequenced(packets);
    }

    async fn send_pending(&mut self) {
        let now = Instant::now();
        if now > self.next_resend && self.out_seq_sent > self.out_seq_acked {
            if self.state == State::Disconnecting {
                self.out_packets.clear();
            }
            let resend: Vec<_> = self.out_packets.iter().take(RESEND_COUNT).cloned().collect();
            for packet in resend {
                self.send_packet(packet).await;
            }
            self.next_resend = Instant::now() + RESEND_DELAY;
        } else if self.out_seq_sent < self.out_seq_acked + AHEAD_COUNT {
            let start = (self.out_seq_sent - self.out_seq_acked) as usize;
            let send: Vec<_> = self
                .out_packets
                .iter()
                .skip(start)
                .take((AHEAD_COUNT as usize).saturating_sub(start))
                .cloned()
                .collect();
            for packet in send {
                self.send_packet(packet).await;
            }
        }
    }

    /// Delivers complete messages in order.
    fn dispatch(&mut self) {
        loop {
            let Some(first) = self.in_packets.get(&(self.in_seq_handled + 1)) else { return };
            let parts = first.header.packets_in_msg.max(1);
            if !(1..parts).all(|i| self.in_packets.contains_key(&(self.in_seq_handled + 1 + i))) {
                return;
            }
            let mut message = Vec::new();
            for _ in 0..parts {
                self.in_seq_handled += 1;
                if let Some(p) = self.in_packets.remove(&self.in_seq_handled) {
                    message.extend_from_slice(&p.payload);
                }
            }
            if self.deliver.send(Bytes::from(message)).is_err() {
                self.begin_disconnect();
                return;
            }
        }
    }

    fn begin_disconnect(&mut self) {
        if matches!(self.state, State::Disconnected | State::Disconnecting) {
            return;
        }
        self.state = State::Disconnecting;
        // The server acks this, so it's sequenced.
        self.send_sequenced(vec![UdpPacket::new(EUdpPacketType::Disconnect)]);
    }

    async fn receive(&mut self, packet: UdpPacket) {
        let h = &packet.header;
        tracing::trace!(kind = ?h.packet_type, seq = h.seq_this, ack = h.seq_ack, parts = h.packets_in_msg, "UDP receive");
        if self.remote_conn_id > 0 && h.source_conn_id != self.remote_conn_id {
            return;
        }
        // A duplicate: ack it again in case the ack was lost.
        if h.packet_type == EUdpPacketType::Data && h.seq_this < self.in_seq {
            self.send_packet(UdpPacket::new(EUdpPacketType::Datagram)).await;
            return;
        }
        if self.out_seq_acked < h.seq_ack {
            self.out_seq_acked = h.seq_ack;
            self.out_seq_sent = self.out_seq_sent.max(self.out_seq_acked);
            let acked = self.out_seq_acked;
            self.out_packets.retain(|p| p.header.seq_this > acked);
            self.next_resend = Instant::now() + RESEND_DELAY;
        }
        if h.seq_this == self.in_seq + 1 {
            self.in_seq += 1;
            while self.in_packets.contains_key(&(self.in_seq + 1)) {
                self.in_seq += 1;
            }
        }

        match h.packet_type {
            EUdpPacketType::Challenge => {
                if self.state != State::ChallengeReqSent {
                    return;
                }
                let Ok(challenge) = ChallengeData::decode(&mut Reader::new(&packet.payload)) else { return };
                self.state = State::ConnectSent;
                let connect = ConnectData { challenge_value: challenge.challenge_value ^ ConnectData::CHALLENGE_MASK };
                let packet_seq = h.seq_this;
                let p = UdpPacket::with_payload(EUdpPacketType::Connect, connect.to_bytes()).expect("small");
                self.send_sequenced(vec![p]);
                self.in_seq_handled = packet_seq;
            }
            EUdpPacketType::Accept => {
                if self.state != State::ConnectSent {
                    return;
                }
                self.state = State::Connected;
                self.remote_conn_id = h.source_conn_id;
                self.in_seq_handled = h.seq_this;
                if let Some(tx) = self.connected.take() {
                    let _ = tx.send(());
                }
            }
            EUdpPacketType::Data => {
                if !matches!(self.state, State::Connected | State::Disconnecting) {
                    return;
                }
                let seq = h.seq_this;
                if seq <= self.in_seq_handled || self.in_packets.contains_key(&seq) {
                    return;
                }
                self.in_packets.insert(seq, packet);
                // in_seq may now advance past packets that arrived early.
                while self.in_packets.contains_key(&(self.in_seq + 1)) {
                    self.in_seq += 1;
                }
                self.dispatch();
            }
            EUdpPacketType::Disconnect => self.state = State::Disconnected,
            _ => {}
        }
    }

    async fn run(mut self, mut out_rx: mpsc::UnboundedReceiver<Outgoing>) {
        self.send_packet(UdpPacket::new(EUdpPacketType::ChallengeReq)).await;
        let mut buf = vec![0u8; 2048];
        let mut out_open = true;
        while self.state != State::Disconnected {
            tokio::select! {
                received = self.sock.recv(&mut buf) => match received {
                    Ok(n) => {
                        self.timeout = Instant::now() + TIMEOUT_DELAY;
                        if let Some(p) = UdpPacket::parse(&buf[..n]) {
                            self.receive(p).await;
                        }
                        // Handle everything that's already here before acking.
                        while let Ok(n) = self.sock.try_recv(&mut buf) {
                            if let Some(p) = UdpPacket::parse(&buf[..n]) {
                                self.receive(p).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("UDP receive failed: {e}");
                        self.state = State::Disconnected;
                    }
                },
                msg = out_rx.recv(), if out_open => match msg {
                    Some(Outgoing::Data(data)) if self.state == State::Connected => self.send_data(&data),
                    Some(Outgoing::Data(_)) => tracing::debug!("dropping UDP message sent before connecting"),
                    Some(Outgoing::Close) | None => {
                        out_open = false;
                        self.begin_disconnect();
                    }
                },
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
            if Instant::now() > self.timeout {
                tracing::debug!("UDP connection timed out");
                break;
            }
            if self.state == State::Disconnected {
                break;
            }
            self.send_pending().await;
            if self.in_seq != self.in_seq_acked {
                self.send_packet(UdpPacket::new(EUdpPacketType::Datagram)).await;
            }
            if self.state == State::Disconnecting && self.out_packets.is_empty() {
                break;
            }
        }
        SOURCE_CONN_ID.fetch_add(256, Ordering::Relaxed);
    }
}

/// Connects and returns the plain message link once the server accepts.
pub(super) async fn connect(endpoint: &str) -> Result<Connection> {
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    // Connecting also filters out datagrams from other senders.
    sock.connect(endpoint).await?;
    let local_ip = sock.local_addr().ok().map(|a| a.ip());

    let (out_tx, out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Bytes>();
    let (connected_tx, connected_rx) = oneshot::channel();
    let now = Instant::now();
    let link = Link {
        sock,
        state: State::ChallengeReqSent,
        source_conn_id: SOURCE_CONN_ID.load(Ordering::Relaxed),
        remote_conn_id: 0,
        out_seq: 1,
        out_seq_sent: 0,
        out_seq_acked: 0,
        in_seq: 0,
        in_seq_acked: 0,
        in_seq_handled: 0,
        out_packets: VecDeque::new(),
        in_packets: HashMap::new(),
        timeout: now + TIMEOUT_DELAY,
        next_resend: now + RESEND_DELAY,
        deliver: in_tx,
        connected: Some(connected_tx),
    };
    tokio::spawn(link.run(out_rx));
    connected_rx.await.map_err(|_| Error::Other("UDP connection was not accepted".into()))?;
    tracing::debug!("UDP connection accepted");
    Ok(Connection { outgoing: out_tx, incoming: in_rx, local_ip })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> UdpHeader {
        UdpHeader {
            magic: UdpHeader::MAGIC,
            payload_size: 0x0102,
            packet_type: EUdpPacketType::Data,
            flags: 0x04,
            source_conn_id: 0x0506_0708,
            dest_conn_id: 0x090A_0B0C,
            seq_this: 0x0D0E_0F10,
            seq_ack: 0x1112_1314,
            packets_in_msg: 0x1516_1718,
            msg_start_seq: 0x191A_1B1C,
            msg_size: 0x1D1E_1F20,
        }
    }

    #[test]
    fn header_layout() {
        let bytes = sample_header().to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        assert_eq!(&bytes[0..4], b"VS01");
        assert_eq!(bytes[4..6], [0x02, 0x01]);
        assert_eq!(bytes[6], 6);
        assert_eq!(bytes[7], 0x04);
        assert_eq!(bytes[8..12], [0x08, 0x07, 0x06, 0x05]);
        assert_eq!(bytes[32..36], [0x20, 0x1F, 0x1E, 0x1D]);
        assert_eq!(UdpHeader::decode(&mut Reader::new(&bytes)).unwrap(), sample_header());
        for len in 0..HEADER_SIZE {
            assert!(UdpHeader::decode(&mut Reader::new(&bytes[..len])).is_err());
        }
        assert_eq!(UdpHeader::default().source_conn_id, 512);
    }

    #[test]
    fn packets() {
        let payload: Vec<u8> = (0..300).map(|i| i as u8).collect();
        let mut packet = UdpPacket::with_payload(EUdpPacketType::Data, payload.clone()).unwrap();
        packet.header.seq_this = 7;
        let parsed = UdpPacket::parse(&packet.to_bytes()).unwrap();
        assert_eq!(parsed.payload, payload);
        assert_eq!(parsed.header.seq_this, 7);
        assert_eq!(parsed.header.payload_size, 300);
        assert_eq!(parsed.header.msg_size, 300);

        assert!(UdpPacket::with_payload(EUdpPacketType::Data, vec![0; MAX_PAYLOAD]).is_ok());
        assert!(UdpPacket::with_payload(EUdpPacketType::Data, vec![0; MAX_PAYLOAD + 1]).is_err());

        let mut bad = packet.to_bytes();
        bad[0] ^= 0xFF;
        assert!(UdpPacket::parse(&bad).is_none());
        assert!(UdpPacket::parse(&packet.to_bytes()[..10]).is_none());
    }

    /// A minimal server: handshake, then echo every message back as two
    /// packets, out of order.
    async fn fake_server(sock: UdpSocket) {
        let mut buf = vec![0u8; 2048];
        let (n, client) = sock.recv_from(&mut buf).await.unwrap();
        let req = UdpPacket::parse(&buf[..n]).unwrap();
        assert_eq!(req.header.packet_type, EUdpPacketType::ChallengeReq);

        let server_id = 0x1234;
        let mut seq = 1;
        let mut send = |mut p: UdpPacket, ack: u32| {
            p.header.source_conn_id = server_id;
            p.header.seq_this = seq;
            p.header.seq_ack = ack;
            p.header.packets_in_msg = p.header.packets_in_msg.max(1);
            seq += 1;
            p.to_bytes()
        };
        let challenge = ChallengeData { challenge_value: 0xCAFE, server_load: 0 }.to_bytes();
        let p = send(UdpPacket::with_payload(EUdpPacketType::Challenge, challenge).unwrap(), 0);
        sock.send_to(&p, client).await.unwrap();

        let (n, _) = sock.recv_from(&mut buf).await.unwrap();
        let connect = UdpPacket::parse(&buf[..n]).unwrap();
        assert_eq!(connect.header.packet_type, EUdpPacketType::Connect);
        let data = ConnectData::decode(&mut Reader::new(&connect.payload)).unwrap();
        assert_eq!(data.challenge_value, 0xCAFE ^ ConnectData::CHALLENGE_MASK);
        let p = send(UdpPacket::new(EUdpPacketType::Accept), connect.header.seq_this);
        sock.send_to(&p, client).await.unwrap();

        // Echo messages until the client disconnects.
        let mut parts: Vec<u8> = Vec::new();
        loop {
            let (n, _) = sock.recv_from(&mut buf).await.unwrap();
            let p = UdpPacket::parse(&buf[..n]).unwrap();
            match p.header.packet_type {
                EUdpPacketType::Data => {
                    assert_eq!(p.header.dest_conn_id, server_id);
                    let last_ack = p.header.seq_this;
                    parts.extend_from_slice(&p.payload);
                    if p.header.seq_this + 1 - p.header.msg_start_seq < p.header.packets_in_msg {
                        continue;
                    }
                    let message = std::mem::take(&mut parts);
                    let half = message.len() / 2;
                    let mut a = UdpPacket::with_payload(EUdpPacketType::Data, message[..half].to_vec()).unwrap();
                    let mut b = UdpPacket::with_payload(EUdpPacketType::Data, message[half..].to_vec()).unwrap();
                    for x in [&mut a, &mut b] {
                        x.header.packets_in_msg = 2;
                        x.header.msg_size = message.len() as u32;
                    }
                    let a = send(a, last_ack);
                    let b = send(b, last_ack);
                    sock.send_to(&b, client).await.unwrap();
                    sock.send_to(&a, client).await.unwrap();
                }
                EUdpPacketType::Disconnect => {
                    let p = send(UdpPacket::new(EUdpPacketType::Datagram), p.header.seq_this);
                    sock.send_to(&p, client).await.unwrap();
                    return;
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn loopback_connection() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap().to_string();
        let server = tokio::spawn(fake_server(server));

        let mut conn = connect(&addr).await.unwrap();
        // A message spanning several packets.
        let message: Vec<u8> = (0..2400u32).map(|i| (i % 251) as u8).collect();
        conn.outgoing.send(Outgoing::Data(message.clone())).unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(5), conn.incoming.recv()).await.unwrap().unwrap();
        assert_eq!(&echoed[..], &message[..]);

        conn.outgoing.send(Outgoing::Close).unwrap();
        tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
    }
}
