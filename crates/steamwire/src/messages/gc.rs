//! Game coordinator framing. GC messages travel inside `CMsgGCClient` and have
//! their own headers: [`MsgGcHdr`] (18 bytes, no `EMsg`) or a protobuf header
//! from `gc.proto` (which differs from the CM one).

use bytes::{Buf, BufMut, Bytes};
use prost::Message;
use steamwire_protos::gc::CMsgProtoBufHeader as GcProtoHeader;

use super::PROTO_MASK;
use crate::error::{Error, Result};
use crate::types::JobId;

/// The plain GC header (`MsgGCHdr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsgGcHdr {
    /// Header version (1).
    pub header_version: u16,
    /// Target job.
    pub target_job: JobId,
    /// Source job.
    pub source_job: JobId,
}

impl Default for MsgGcHdr {
    fn default() -> Self {
        Self { header_version: 1, target_job: JobId::NONE, source_job: JobId::NONE }
    }
}

/// The header of a GC message.
#[derive(Clone, Debug, PartialEq)]
pub enum GcHeader {
    /// A plain header.
    Plain(MsgGcHdr),
    /// A protobuf header.
    Proto(Box<GcProtoHeader>),
}

/// A game coordinator message.
///
/// For plain messages, the GC's `msg_type` from `CMsgGCClient` is the type;
/// there's no type in the payload. For protobuf messages the payload starts
/// with the type, masked with [`PROTO_MASK`].
#[derive(Clone, Debug)]
pub struct GcPacket {
    /// The app-specific message type, without [`PROTO_MASK`].
    pub msg_type: u32,
    /// The header.
    pub header: GcHeader,
    /// The body.
    pub body: Bytes,
}

impl GcPacket {
    /// Parses a GC payload. `msg_type` is `CMsgGCClient::msgtype`.
    pub fn parse(msg_type: u32, mut data: Bytes) -> Result<GcPacket> {
        let real_type = msg_type & !PROTO_MASK;
        if msg_type & PROTO_MASK != 0 {
            if data.len() < 8 {
                return Err(Error::Malformed("short GC protobuf header"));
            }
            let _type = data.get_u32_le();
            let len = data.get_i32_le();
            let len = usize::try_from(len).map_err(|_| Error::Malformed("negative GC header length"))?;
            if data.len() < len {
                return Err(Error::Malformed("GC header longer than packet"));
            }
            let header = GcProtoHeader::decode(data.split_to(len))?;
            Ok(GcPacket { msg_type: real_type, header: GcHeader::Proto(Box::new(header)), body: data })
        } else {
            if data.len() < 18 {
                return Err(Error::Malformed("short GC header"));
            }
            let header = MsgGcHdr {
                header_version: data.get_u16_le(),
                target_job: JobId(data.get_u64_le()),
                source_job: JobId(data.get_u64_le()),
            };
            Ok(GcPacket { msg_type: real_type, header: GcHeader::Plain(header), body: data })
        }
    }

    /// Whether the message has a protobuf header.
    pub fn is_proto(&self) -> bool {
        matches!(self.header, GcHeader::Proto(_))
    }

    /// The protobuf header, if any.
    pub fn proto_header(&self) -> Option<&GcProtoHeader> {
        match &self.header {
            GcHeader::Proto(h) => Some(h),
            GcHeader::Plain(_) => None,
        }
    }

    /// The job this message answers.
    pub fn target_job(&self) -> JobId {
        match &self.header {
            GcHeader::Plain(h) => h.target_job,
            GcHeader::Proto(h) => JobId(h.job_id_target()),
        }
    }

    /// The job that sent this message.
    pub fn source_job(&self) -> JobId {
        match &self.header {
            GcHeader::Plain(h) => h.source_job,
            GcHeader::Proto(h) => JobId(h.job_id_source()),
        }
    }

    /// Decodes the body as a protobuf message.
    pub fn decode<M: Message + Default>(&self) -> Result<M> {
        Ok(M::decode(self.body.clone())?)
    }

    /// The `msgtype` for `CMsgGCClient` (masked for protobuf messages).
    pub fn wire_type(&self) -> u32 {
        if self.is_proto() {
            self.msg_type | PROTO_MASK
        } else {
            self.msg_type
        }
    }

    /// Encodes the payload for `CMsgGCClient::payload`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 32);
        match &self.header {
            GcHeader::Proto(h) => {
                out.put_u32_le(self.msg_type | PROTO_MASK);
                out.put_i32_le(h.encoded_len() as i32);
                h.encode(&mut out).expect("Vec grows");
            }
            GcHeader::Plain(h) => {
                out.put_u16_le(h.header_version);
                out.put_u64_le(h.target_job.0);
                out.put_u64_le(h.source_job.0);
            }
        }
        out.extend_from_slice(&self.body);
        out
    }

    /// A protobuf GC message.
    pub fn proto<M: Message>(msg_type: u32, body: &M) -> GcPacket {
        GcPacket {
            msg_type: msg_type & !PROTO_MASK,
            header: GcHeader::Proto(Box::default()),
            body: Bytes::from(body.encode_to_vec()),
        }
    }

    /// A plain GC message with a raw body.
    pub fn plain(msg_type: u32, body: impl Into<Bytes>) -> GcPacket {
        GcPacket { msg_type, header: GcHeader::Plain(MsgGcHdr::default()), body: body.into() }
    }

    /// Sets the source job.
    pub fn set_source_job(&mut self, job: JobId) {
        match &mut self.header {
            GcHeader::Plain(h) => h.source_job = job,
            GcHeader::Proto(h) => h.job_id_source = Some(job.0),
        }
    }

    /// Sets the target job (for replies).
    pub fn set_target_job(&mut self, job: JobId) {
        match &mut self.header {
            GcHeader::Plain(h) => h.target_job = job,
            GcHeader::Proto(h) => h.job_id_target = Some(job.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proto_round_trip() {
        let body = steamwire_protos::gc::CMsgProtoBufHeader { client_session_id: Some(3), ..Default::default() };
        let mut packet = GcPacket::proto(4006, &body);
        packet.set_source_job(JobId(9));
        let wire = packet.encode();
        let back = GcPacket::parse(packet.wire_type(), Bytes::from(wire)).unwrap();
        assert_eq!(back.msg_type, 4006);
        assert_eq!(back.source_job(), JobId(9));
        assert_eq!(back.decode::<steamwire_protos::gc::CMsgProtoBufHeader>().unwrap(), body);
    }

    #[test]
    fn plain_round_trip() {
        let mut packet = GcPacket::plain(4004, &b"hi"[..]);
        packet.set_target_job(JobId(5));
        let back = GcPacket::parse(4004, Bytes::from(packet.encode())).unwrap();
        assert!(!back.is_proto());
        assert_eq!(back.target_job(), JobId(5));
        assert_eq!(&back.body[..], b"hi");
    }
}
