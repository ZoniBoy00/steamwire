//! Non-protobuf message bodies: the `Msg*` structs generated from
//! SteamKit2's `steammsg.steamd` / `netheader.steamd`, and [`ClientMsg`],
//! the message type that carries them.

use bytes::Bytes;

use super::{ExtendedClientMsgHdr, Header, Packet};
use crate::enums::EMsg;
use crate::error::{Error, Result};
use crate::types::{JobId, SteamId};

/// A fixed-layout little-endian struct.
pub trait SteamStruct: Sized {
    /// Appends the encoded struct.
    fn encode(&self, out: &mut Vec<u8>);
    /// Reads the struct.
    fn decode(r: &mut Reader<'_>) -> Result<Self>;

    /// Encodes into a new buffer.
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// A [`SteamStruct`] that is the body of a message type.
pub trait MsgBody: SteamStruct {
    /// The message type.
    const EMSG: EMsg;
}

/// A little-endian reader over a byte slice.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
}

macro_rules! read_int {
    ($($name:ident: $t:ty),*) => {$(
        #[doc = concat!("Reads a `", stringify!($t), "`.")]
        pub fn $name(&mut self) -> Result<$t> {
            Ok(<$t>::from_le_bytes(self.array()?))
        }
    )*};
}

impl<'a> Reader<'a> {
    /// A reader over `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    /// The unread bytes.
    pub fn remaining(&self) -> &'a [u8] {
        self.buf
    }

    /// Reads `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(Error::Malformed("struct shorter than its layout"));
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    /// Reads a fixed-size array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    /// Reads a NUL-terminated UTF-8 string (lossy).
    pub fn cstring(&mut self) -> Result<String> {
        let end = self.buf.iter().position(|&b| b == 0).ok_or(Error::Malformed("unterminated string"))?;
        let s = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        self.buf = &self.buf[end + 1..];
        Ok(s)
    }

    /// Reads a `u8`.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    read_int!(i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32);
}

/// A non-protobuf client message: an [`ExtendedClientMsgHdr`], a fixed body
/// and an optional free-form payload after it.
#[derive(Clone, Debug)]
pub struct ClientMsg<T> {
    /// The header.
    pub header: ExtendedClientMsgHdr,
    /// The body.
    pub body: T,
    /// Bytes after the body (strings, blobs).
    pub payload: Vec<u8>,
}

impl<T: MsgBody> ClientMsg<T> {
    /// A message with a default header.
    pub fn new(body: T) -> Self {
        Self {
            header: ExtendedClientMsgHdr {
                target_job: JobId::NONE,
                source_job: JobId::NONE,
                steam_id: SteamId::default(),
                session_id: 0,
            },
            body,
            payload: Vec::new(),
        }
    }

    /// Reads a message from a packet with an extended header.
    pub fn from_packet(packet: &Packet) -> Result<Self> {
        if packet.emsg != T::EMSG {
            return Err(Error::UnexpectedMessage(packet.emsg));
        }
        let Header::Extended(header) = &packet.header else {
            return Err(Error::Malformed("expected an extended client header"));
        };
        let mut r = Reader::new(&packet.body);
        let body = T::decode(&mut r)?;
        Ok(Self { header: *header, body, payload: r.remaining().to_vec() })
    }

    /// Converts to a packet.
    pub fn to_packet(&self) -> Packet {
        let mut body = self.body.to_bytes();
        body.extend_from_slice(&self.payload);
        Packet { emsg: T::EMSG, header: Header::Extended(self.header), body: Bytes::from(body) }
    }
}

impl<T: MsgBody> super::OutgoingMsg for ClientMsg<T> {
    fn emsg(&self) -> EMsg {
        T::EMSG
    }

    fn set_source_job(&mut self, job: JobId) {
        self.header.source_job = job;
    }

    fn fill_session(&mut self, steam_id: Option<SteamId>, session_id: Option<i32>) {
        if let Some(id) = steam_id {
            if self.header.steam_id == SteamId::default() {
                self.header.steam_id = id;
            }
        }
        if let Some(session) = session_id {
            if self.header.session_id == 0 {
                self.header.session_id = session;
            }
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        self.to_packet().encode()
    }
}

/// The generated structs.
#[allow(non_snake_case, clippy::all, deprecated, missing_docs)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}

#[cfg(test)]
mod tests {
    use super::generated::*;
    use super::*;
    use crate::enums::{EChatEntryType, EResult};

    #[test]
    fn struct_round_trip() {
        let msg = MsgClientChatMsg {
            steam_id_chatter: SteamId::from_account_id(1),
            steam_id_chat_room: SteamId::from_u64(110338190870577153),
            chat_msg_type: EChatEntryType::ChatMsg,
        };
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 20);
        assert_eq!(MsgClientChatMsg::decode(&mut Reader::new(&bytes)).unwrap(), msg);
    }

    #[test]
    fn defaults_and_consts() {
        let req = MsgChannelEncryptResponse::default();
        assert_eq!(req.protocol_version, MsgChannelEncryptRequest::PROTOCOL_VERSION);
        assert_eq!(req.key_size, 128);
        assert_eq!(MsgChannelEncryptResult::default().result, EResult::Invalid);
        assert_eq!(MsgClientLogon::CurrentProtocol, 65581);
        assert_eq!(MsgClientLogon::ObfuscationMask, 0xBAAD_F00D);
        assert_eq!(UdpHeader::default().magic, UdpHeader::MAGIC);
        assert_eq!(UdpHeader::default().to_bytes().len(), 36);
    }

    #[test]
    fn client_msg_round_trip() {
        let mut msg = ClientMsg::new(MsgClientLoggedOff {
            result: EResult::LogonSessionReplaced,
            sec_min_reconnect_hint: 5,
            sec_max_reconnect_hint: 10,
        });
        msg.payload = b"extra".to_vec();
        let packet = Packet::parse(Bytes::from(msg.to_packet().encode())).unwrap();
        let back = ClientMsg::<MsgClientLoggedOff>::from_packet(&packet).unwrap();
        assert_eq!(back.body, msg.body);
        assert_eq!(back.payload, b"extra");
    }
}
