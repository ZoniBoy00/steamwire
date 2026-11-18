//! CM message framing.
//!
//! Every CM message starts with a little-endian `u32` [`EMsg`]. Its top bit
//! ([`PROTO_MASK`]) says which header follows:
//!
//! * set: `i32` header length + a [`CMsgProtoBufHeader`], then a protobuf body;
//! * clear, for the `ChannelEncrypt*` messages: [`MsgHdr`] (20 bytes);
//! * clear, otherwise: [`ExtendedClientMsgHdr`] (36 bytes).

use bytes::{Buf, BufMut, Bytes};
use prost::Message;
use steamkit_protos::steam::CMsgProtoBufHeader;

use crate::enums::{EMsg, EResult};
use crate::error::{Error, Result};
use crate::types::{JobId, SteamId};

/// The bit set on the `EMsg` of protobuf messages.
pub const PROTO_MASK: u32 = 0x8000_0000;

/// The plain header used by the channel encryption handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgHdr {
    /// Target job.
    pub target_job: JobId,
    /// Source job.
    pub source_job: JobId,
}

impl MsgHdr {
    /// Encoded length, including the `EMsg`.
    pub const LEN: usize = 20;
}

/// The header of non-protobuf client messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtendedClientMsgHdr {
    /// Target job.
    pub target_job: JobId,
    /// Source job.
    pub source_job: JobId,
    /// Steam ID of the session.
    pub steam_id: SteamId,
    /// Session ID.
    pub session_id: i32,
}

impl ExtendedClientMsgHdr {
    /// Encoded length, including the `EMsg`.
    pub const LEN: usize = 36;
    const HEADER_SIZE: u8 = 36;
    const HEADER_VERSION: u16 = 2;
    const CANARY: u8 = 239;
}

/// The header of a received message.
#[derive(Clone, Debug, PartialEq)]
pub enum Header {
    /// [`MsgHdr`].
    Plain(MsgHdr),
    /// [`ExtendedClientMsgHdr`].
    Extended(ExtendedClientMsgHdr),
    /// A protobuf header.
    Proto(Box<CMsgProtoBufHeader>),
}

/// A message received from (or about to be sent to) a CM server.
#[derive(Clone, Debug)]
pub struct Packet {
    /// The message type, without [`PROTO_MASK`].
    pub emsg: EMsg,
    /// The header.
    pub header: Header,
    /// The body, after the header.
    pub body: Bytes,
}

fn uses_plain_header(emsg: EMsg) -> bool {
    matches!(emsg, EMsg::ChannelEncryptRequest | EMsg::ChannelEncryptResponse | EMsg::ChannelEncryptResult)
}

impl Packet {
    /// Parses a raw message.
    pub fn parse(mut data: Bytes) -> Result<Packet> {
        if data.len() < 4 {
            return Err(Error::Malformed("packet shorter than its EMsg"));
        }
        let raw = data.get_u32_le();
        let emsg = EMsg(raw & !PROTO_MASK);

        let header = if raw & PROTO_MASK != 0 {
            if data.len() < 4 {
                return Err(Error::Malformed("missing protobuf header length"));
            }
            let len = data.get_i32_le();
            let len = usize::try_from(len).map_err(|_| Error::Malformed("negative header length"))?;
            if data.len() < len {
                return Err(Error::Malformed("protobuf header longer than packet"));
            }
            Header::Proto(Box::new(CMsgProtoBufHeader::decode(data.split_to(len))?))
        } else if uses_plain_header(emsg) {
            if data.len() < MsgHdr::LEN - 4 {
                return Err(Error::Malformed("short MsgHdr"));
            }
            Header::Plain(MsgHdr { target_job: JobId(data.get_u64_le()), source_job: JobId(data.get_u64_le()) })
        } else {
            if data.len() < ExtendedClientMsgHdr::LEN - 4 {
                return Err(Error::Malformed("short ExtendedClientMsgHdr"));
            }
            let _header_size = data.get_u8();
            let _header_version = data.get_u16_le();
            let target_job = JobId(data.get_u64_le());
            let source_job = JobId(data.get_u64_le());
            let _canary = data.get_u8();
            let steam_id = SteamId::from_u64(data.get_u64_le());
            let session_id = data.get_i32_le();
            Header::Extended(ExtendedClientMsgHdr { target_job, source_job, steam_id, session_id })
        };

        Ok(Packet { emsg, header, body: data })
    }

    /// Whether the message has a protobuf header.
    pub fn is_proto(&self) -> bool {
        matches!(self.header, Header::Proto(_))
    }

    /// The protobuf header, if any.
    pub fn proto_header(&self) -> Option<&CMsgProtoBufHeader> {
        match &self.header {
            Header::Proto(h) => Some(h),
            _ => None,
        }
    }

    /// The job this message answers.
    pub fn target_job(&self) -> JobId {
        match &self.header {
            Header::Plain(h) => h.target_job,
            Header::Extended(h) => h.target_job,
            Header::Proto(h) => JobId(h.jobid_target()),
        }
    }

    /// The job that sent this message.
    pub fn source_job(&self) -> JobId {
        match &self.header {
            Header::Plain(h) => h.source_job,
            Header::Extended(h) => h.source_job,
            Header::Proto(h) => JobId(h.jobid_source()),
        }
    }

    /// The `EResult` from the protobuf header (`OK` for other headers isn't
    /// implied: they return `None`).
    pub fn header_result(&self) -> Option<EResult> {
        self.proto_header().map(|h| EResult(h.eresult()))
    }

    /// The Steam ID in the header.
    pub fn steam_id(&self) -> Option<SteamId> {
        match &self.header {
            Header::Plain(_) => None,
            Header::Extended(h) => Some(h.steam_id),
            Header::Proto(h) => h.steamid.map(SteamId::from_u64),
        }
    }

    /// The session ID in the header.
    pub fn session_id(&self) -> Option<i32> {
        match &self.header {
            Header::Plain(_) => None,
            Header::Extended(h) => Some(h.session_id),
            Header::Proto(h) => h.client_sessionid,
        }
    }

    /// Decodes the body as a protobuf message.
    pub fn decode<M: Message + Default>(&self) -> Result<M> {
        Ok(M::decode(self.body.clone())?)
    }

    /// Encodes the packet back to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 64);
        match &self.header {
            Header::Proto(h) => {
                out.put_u32_le(self.emsg.0 | PROTO_MASK);
                out.put_i32_le(h.encoded_len() as i32);
                h.encode(&mut out).expect("BytesMut grows");
            }
            Header::Plain(h) => {
                out.put_u32_le(self.emsg.0);
                out.put_u64_le(h.target_job.0);
                out.put_u64_le(h.source_job.0);
            }
            Header::Extended(h) => {
                out.put_u32_le(self.emsg.0);
                out.put_u8(ExtendedClientMsgHdr::HEADER_SIZE);
                out.put_u16_le(ExtendedClientMsgHdr::HEADER_VERSION);
                out.put_u64_le(h.target_job.0);
                out.put_u64_le(h.source_job.0);
                out.put_u8(ExtendedClientMsgHdr::CANARY);
                out.put_u64_le(h.steam_id.as_u64());
                out.put_i32_le(h.session_id);
            }
        }
        out.extend_from_slice(&self.body);
        out
    }
}

/// A message that [`SteamClient::send`](crate::SteamClient::send) can send.
pub trait OutgoingMsg {
    /// The message type.
    fn emsg(&self) -> EMsg;
    /// Sets the source job, for messages sent as jobs.
    fn set_source_job(&mut self, job: JobId);
    /// Fills in the session's Steam ID and session ID unless already set.
    fn fill_session(&mut self, steam_id: Option<SteamId>, session_id: Option<i32>);
    /// Encodes the message.
    fn to_bytes(&self) -> Vec<u8>;
}

impl<M: Message> OutgoingMsg for ProtoMsg<M> {
    fn emsg(&self) -> EMsg {
        self.emsg
    }

    fn set_source_job(&mut self, job: JobId) {
        self.header.jobid_source = Some(job.0);
    }

    fn fill_session(&mut self, steam_id: Option<SteamId>, session_id: Option<i32>) {
        if self.header.steamid.is_none() {
            self.header.steamid = steam_id.map(SteamId::as_u64);
        }
        if self.header.client_sessionid.is_none() {
            self.header.client_sessionid = session_id;
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        self.encode()
    }
}

impl OutgoingMsg for Packet {
    fn emsg(&self) -> EMsg {
        self.emsg
    }

    fn set_source_job(&mut self, job: JobId) {
        match &mut self.header {
            Header::Plain(h) => h.source_job = job,
            Header::Extended(h) => h.source_job = job,
            Header::Proto(h) => h.jobid_source = Some(job.0),
        }
    }

    fn fill_session(&mut self, steam_id: Option<SteamId>, session_id: Option<i32>) {
        match &mut self.header {
            Header::Plain(_) => {}
            Header::Extended(h) => {
                if h.steam_id == SteamId::default() {
                    h.steam_id = steam_id.unwrap_or_default();
                }
                if h.session_id == 0 {
                    h.session_id = session_id.unwrap_or(0);
                }
            }
            Header::Proto(h) => {
                if h.steamid.is_none() {
                    h.steamid = steam_id.map(SteamId::as_u64);
                }
                if h.client_sessionid.is_none() {
                    h.client_sessionid = session_id;
                }
            }
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        self.encode()
    }
}

/// An outgoing protobuf message.
#[derive(Clone, Debug)]
pub struct ProtoMsg<M> {
    /// The message type.
    pub emsg: EMsg,
    /// The protobuf header. The client fills in the Steam ID and session ID.
    pub header: CMsgProtoBufHeader,
    /// The body.
    pub body: M,
}

impl<M: Message> ProtoMsg<M> {
    /// A message with an empty header.
    pub fn new(emsg: EMsg, body: M) -> Self {
        Self { emsg, header: CMsgProtoBufHeader::default(), body }
    }

    /// Encodes the message.
    pub fn encode(&self) -> Vec<u8> {
        let header_len = self.header.encoded_len();
        let mut out = Vec::with_capacity(8 + header_len + self.body.encoded_len());
        out.put_u32_le(self.emsg.0 | PROTO_MASK);
        out.put_i32_le(header_len as i32);
        self.header.encode(&mut out).expect("Vec grows");
        self.body.encode(&mut out).expect("Vec grows");
        out
    }
}

/// Splits the payload of an `EMsg::Multi` message into its packets.
pub fn split_multi(packet: &Packet) -> Result<Vec<Packet>> {
    split_multi_raw(packet)?.into_iter().map(Packet::parse).collect()
}

/// Splits the payload of an `EMsg::Multi` message into the raw messages.
pub fn split_multi_raw(packet: &Packet) -> Result<Vec<Bytes>> {
    use std::io::Read;

    let multi: steamkit_protos::steam::CMsgMulti = packet.decode()?;
    let size_unzipped = multi.size_unzipped();
    let body = multi.message_body.unwrap_or_default();
    let payload = if size_unzipped > 0 {
        let mut out = Vec::with_capacity(size_unzipped as usize);
        flate2::read::GzDecoder::new(body.as_slice()).read_to_end(&mut out)?;
        out
    } else {
        body
    };

    let mut payload = Bytes::from(payload);
    let mut messages = Vec::new();
    while payload.remaining() >= 4 {
        let len = payload.get_u32_le() as usize;
        if payload.len() < len {
            return Err(Error::Malformed("truncated sub-message in Multi"));
        }
        messages.push(payload.split_to(len));
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use steamkit_protos::steam::{CMsgClientHeartBeat, CMsgMulti};

    #[test]
    fn proto_round_trip() {
        let mut msg = ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat { send_reply: Some(true) });
        msg.header.steamid = Some(76561197960287930);
        msg.header.jobid_source = Some(42);
        let packet = Packet::parse(Bytes::from(msg.encode())).unwrap();
        assert_eq!(packet.emsg, EMsg::ClientHeartBeat);
        assert!(packet.is_proto());
        assert_eq!(packet.source_job(), JobId(42));
        assert_eq!(packet.target_job(), JobId::NONE);
        assert_eq!(packet.steam_id(), Some(SteamId::from_u64(76561197960287930)));
        let body: CMsgClientHeartBeat = packet.decode().unwrap();
        assert_eq!(body.send_reply, Some(true));
        assert_eq!(packet.encode(), msg.encode());
    }

    #[test]
    fn extended_and_plain_round_trip() {
        let packet = Packet {
            emsg: EMsg::ClientLogOnResponse,
            header: Header::Extended(ExtendedClientMsgHdr {
                target_job: JobId(1),
                source_job: JobId::NONE,
                steam_id: SteamId::from_account_id(5),
                session_id: -3,
            }),
            body: Bytes::from_static(b"body"),
        };
        let bytes = packet.encode();
        assert_eq!(bytes.len(), ExtendedClientMsgHdr::LEN + 4);
        let parsed = Packet::parse(Bytes::from(bytes)).unwrap();
        assert_eq!(parsed.header, packet.header);
        assert_eq!(&parsed.body[..], b"body");

        let plain = Packet {
            emsg: EMsg::ChannelEncryptResult,
            header: Header::Plain(MsgHdr { target_job: JobId::NONE, source_job: JobId::NONE }),
            body: Bytes::from_static(&[1, 0, 0, 0]),
        };
        let bytes = plain.encode();
        assert_eq!(bytes.len(), MsgHdr::LEN + 4);
        assert_eq!(Packet::parse(Bytes::from(bytes)).unwrap().header, plain.header);
    }

    #[test]
    fn multi_gzip() {
        use std::io::Write;

        let inner = ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat::default()).encode();
        let mut raw = Vec::new();
        for _ in 0..3 {
            raw.extend_from_slice(&(inner.len() as u32).to_le_bytes());
            raw.extend_from_slice(&inner);
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        let multi = ProtoMsg::new(
            EMsg::Multi,
            CMsgMulti { size_unzipped: Some(raw.len() as u32), message_body: Some(gz.finish().unwrap()) },
        );
        let packet = Packet::parse(Bytes::from(multi.encode())).unwrap();
        let packets = split_multi(&packet).unwrap();
        assert_eq!(packets.len(), 3);
        assert!(packets.iter().all(|p| p.emsg == EMsg::ClientHeartBeat));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Packet::parse(Bytes::from_static(&[1, 2])).is_err());
        assert!(Packet::parse(Bytes::from_static(&[0x8A, 0x15, 0, 0x80, 0xFF, 0, 0, 0])).is_err());
    }
}
