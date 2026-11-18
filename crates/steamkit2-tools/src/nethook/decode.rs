//! Turns a message into a [`Node`] tree (NetHookAnalyzer2's
//! `NetHookItem_MessageReading`, `MessageTypeFinder` and specializations),
//! decoding protobufs by reflection over steamkit2-protos' descriptors.

use bytes::Bytes;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Kind, MapKey, MessageDescriptor, Value};
use steamkit2::kv::KeyValue;
use steamkit2::messages::gc::{GcHeader, GcPacket};
use steamkit2::messages::{generated, split_multi, Header, Packet};
use steamkit2::{EMsg, SteamId};

use crate::tree::{hex_preview, Node};
use crate::wire::varint;

/// Message types whose body isn't `CMsg<EMsg name>`
/// (NetHookAnalyzer2's `MessageTypeOverrides.BodyMap`).
const BODY_OVERRIDES: &[(EMsg, &str)] = &[
    (EMsg::ClientLogonGameServer, "CMsgClientLogon"),
    (EMsg::ClientGamesPlayed, "CMsgClientGamesPlayed"),
    (EMsg::ClientGamesPlayedNoDataBlob, "CMsgClientGamesPlayed"),
    (EMsg::ClientGamesPlayedWithDataBlob, "CMsgClientGamesPlayed"),
    (EMsg::ClientToGC, "CMsgGCClient"),
    (EMsg::ClientFromGC, "CMsgGCClient"),
    (EMsg::ClientFriendMsgIncoming, "CMsgClientFriendMsgIncoming"),
    (EMsg::ClientFriendMsgEchoToSender, "CMsgClientFriendMsgIncoming"),
    (EMsg::ClientCurrentUIMode, "CMsgClientUIMode"),
    (EMsg::ClientGetNumberOfCurrentPlayersDP, "CMsgDPGetNumberOfCurrentPlayers"),
    (EMsg::ClientGetNumberOfCurrentPlayersDPResponse, "CMsgDPGetNumberOfCurrentPlayersResponse"),
    (EMsg::AMGameServerUpdate, "CMsgGameServerData"),
    (EMsg::ClientDPUpdateAppJobReport, "CDataPublisher_ClientUpdateAppJob_Notification"),
    (EMsg::ClientPlayingSessionState, "CMsgClientPlayingSessionState"),
    (EMsg::ClientNetworkingCertRequestResponse, "CMsgClientNetworkingCertReply"),
];

/// Decodes messages into trees.
pub struct Analyzer {
    steam: DescriptorPool,
    gc: DescriptorPool,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    /// An analyzer for the messages known to steamkit2-protos.
    pub fn new() -> Self {
        let steam = DescriptorPool::decode(steamkit2_protos::STEAM_DESCRIPTOR_SET).expect("steam descriptors");
        let gc = DescriptorPool::decode(steamkit2_protos::GC_DESCRIPTOR_SET).expect("gc descriptors");
        Self { steam, gc }
    }

    /// The protobuf body type of `emsg`: an override, `CMsg<name>`, or the
    /// shortest message type whose name ends with the EMsg name.
    pub fn body_type(&self, emsg: EMsg) -> Option<MessageDescriptor> {
        if let Some((_, name)) = BODY_OVERRIDES.iter().find(|(e, _)| *e == emsg) {
            return self.steam.get_message_by_name(name);
        }
        let name = emsg.name()?;
        if let Some(m) = self.steam.get_message_by_name(&format!("CMsg{name}")) {
            return Some(m);
        }
        let lower = name.to_ascii_lowercase();
        self.steam
            .all_messages()
            .filter(|m| m.parent_message().is_none() && m.name().to_ascii_lowercase().ends_with(&lower))
            .min_by_key(|m| m.name().len())
    }

    /// The input (`response: false`) or output type of a service method
    /// named like `Player.GetGameBadgeLevels#1`.
    pub fn service_method_type(&self, name: &str, response: bool) -> Option<MessageDescriptor> {
        let name = name.split('#').next()?;
        let (service, method) = name.split_once('.')?;
        let method = self.steam.get_service_by_name(service)?.methods().find(|m| m.name() == method)?;
        Some(if response { method.output() } else { method.input() })
    }

    /// Decodes a whole message file.
    pub fn message(&self, data: &[u8]) -> Node {
        match Packet::parse(Bytes::copy_from_slice(data)) {
            Ok(packet) => self.packet(&packet),
            Err(e) => Node::branch(
                format!("unparseable message: {e}"),
                vec![Node::leaf(format!("{} bytes: {}", data.len(), hex_preview(data)))],
            ),
        }
    }

    /// Decodes a parsed message.
    pub fn packet(&self, packet: &Packet) -> Node {
        let proto = matches!(packet.header, Header::Proto(_));
        let label = format!("{} ({}){}", packet.emsg, packet.emsg.0, if proto { ", protobuf" } else { "" });
        Node::branch(label, vec![self.header(&packet.header), self.body(packet)])
    }

    fn header(&self, header: &Header) -> Node {
        match header {
            Header::Plain(h) => Node::branch(
                "Header: MsgHdr",
                vec![
                    Node::leaf(format!("target_job: {}", h.target_job.0)),
                    Node::leaf(format!("source_job: {}", h.source_job.0)),
                ],
            ),
            Header::Extended(h) => Node::branch(
                "Header: ExtendedClientMsgHdr",
                vec![
                    Node::leaf(format!("target_job: {}", h.target_job.0)),
                    Node::leaf(format!("source_job: {}", h.source_job.0)),
                    Node::leaf(format!("steam_id: {}", steam_id(h.steam_id.as_u64()))),
                    Node::leaf(format!("session_id: {}", h.session_id)),
                ],
            ),
            Header::Proto(h) => {
                self.encoded(&self.steam, "Header: CMsgProtoBufHeader", "CMsgProtoBufHeader", &h.encode_to_vec())
            }
        }
    }

    fn encoded(&self, pool: &DescriptorPool, label: &str, type_name: &str, data: &[u8]) -> Node {
        match pool.get_message_by_name(type_name) {
            Some(desc) => self.typed(label, &desc, data),
            None => raw_protobuf(label, data),
        }
    }

    /// Decodes `data` as `desc`, falling back to a raw field dump.
    fn typed(&self, label: &str, desc: &MessageDescriptor, data: &[u8]) -> Node {
        match DynamicMessage::decode(desc.clone(), data) {
            Ok(msg) => message_node(label.to_owned(), &msg),
            Err(e) => {
                let mut node = raw_protobuf(label, data);
                node.label = format!("{label} (not a valid {}: {e})", desc.name());
                node
            }
        }
    }

    fn body(&self, packet: &Packet) -> Node {
        let body = &packet.body[..];
        let job_name = match &packet.header {
            Header::Proto(h) => h.target_job_name.clone(),
            _ => None,
        };
        match packet.emsg {
            EMsg::ServiceMethod
            | EMsg::ServiceMethodSendToClient
            | EMsg::ServiceMethodCallFromClient
            | EMsg::ServiceMethodCallFromClientNonAuthed
            | EMsg::ServiceMethodResponse => {
                let response = packet.emsg == EMsg::ServiceMethodResponse;
                let name = job_name.unwrap_or_default();
                return match self.service_method_type(&name, response) {
                    Some(desc) => self.typed(&format!("Body: {} ({name})", desc.name()), &desc, body),
                    None => raw_protobuf(&format!("Body: unknown service method {name:?}"), body),
                };
            }
            EMsg::ClientServiceMethodLegacy => {
                let desc = self.steam.get_message_by_name("CMsgClientServiceMethodLegacy").expect("known type");
                let mut node = self.typed("Body: CMsgClientServiceMethodLegacy", &desc, body);
                if let Ok(msg) = DynamicMessage::decode(desc, body) {
                    let name = msg.get_field_by_name("method_name").and_then(|v| v.as_str().map(str::to_owned));
                    let inner = msg.get_field_by_name("serialized_method").and_then(|v| v.as_bytes().cloned());
                    if let (Some(name), Some(inner)) = (name, inner) {
                        if let Some(method) = self.service_method_type(&name, false) {
                            node.children.push(self.typed(&format!("Method: {}", method.name()), &method, &inner));
                        }
                    }
                }
                return node;
            }
            _ => {}
        }

        if matches!(packet.header, Header::Proto(_)) {
            let mut node = match self.body_type(packet.emsg) {
                Some(desc) => self.typed(&format!("Body: {}", desc.name()), &desc, body),
                None => raw_protobuf("Body: unknown protobuf", body),
            };
            match packet.emsg {
                EMsg::Multi => node.children.push(self.multi(packet)),
                EMsg::ClientToGC | EMsg::ClientFromGC => node.children.push(self.gc(packet)),
                _ => {}
            }
            return node;
        }

        match generated::describe_body(packet.emsg, body) {
            Some((name, Ok((debug, used)))) => {
                let lines = debug.lines().collect::<Vec<_>>();
                let fields = lines.get(1..lines.len().saturating_sub(1)).unwrap_or_default();
                let mut node = Node::branch(
                    format!("Body: {name}"),
                    fields.iter().map(|l| Node::leaf(l.trim().trim_end_matches(','))).collect(),
                );
                if used < body.len() {
                    node.children.push(payload_node(&body[used..]));
                }
                node
            }
            Some((name, Err(e))) => Node::branch(format!("Body: {name} (failed: {e})"), vec![payload_node(body)]),
            None => Node::branch("Body: unknown", vec![payload_node(body)]),
        }
    }

    /// The messages packed in a `Multi`.
    fn multi(&self, packet: &Packet) -> Node {
        match split_multi(packet) {
            Ok(packets) => {
                Node::branch(format!("Messages ({})", packets.len()), packets.iter().map(|p| self.packet(p)).collect())
            }
            Err(e) => Node::leaf(format!("Messages: failed to unpack: {e}")),
        }
    }

    /// The GC message in a `CMsgGCClient`.
    fn gc(&self, packet: &Packet) -> Node {
        let Ok(desc) = self.steam.get_message_by_name("CMsgGCClient").ok_or(()) else {
            return Node::leaf("GC message: no CMsgGCClient type");
        };
        let Ok(msg) = DynamicMessage::decode(desc, &packet.body[..]) else {
            return Node::leaf("GC message: invalid CMsgGCClient");
        };
        let msg_type = msg.get_field_by_name("msgtype").and_then(|v| v.as_u32()).unwrap_or(0);
        let payload = msg.get_field_by_name("payload").and_then(|v| v.as_bytes().cloned()).unwrap_or_default();
        match GcPacket::parse(msg_type, payload) {
            Ok(gc) => {
                let header = match &gc.header {
                    GcHeader::Plain(h) => Node::branch(
                        "Header: MsgGCHdr",
                        vec![
                            Node::leaf(format!("target_job: {}", h.target_job.0)),
                            Node::leaf(format!("source_job: {}", h.source_job.0)),
                        ],
                    ),
                    GcHeader::Proto(h) => {
                        self.encoded(&self.gc, "Header: CMsgProtoBufHeader", "CMsgProtoBufHeader", &h.encode_to_vec())
                    }
                };
                // GC message types are per game; without the game's protos
                // the body is shown field by field.
                let body = if gc.is_proto() {
                    raw_protobuf("Body", &gc.body)
                } else {
                    Node::branch("Body", vec![payload_node(&gc.body)])
                };
                Node::branch(format!("GC message {} ({msg_type:#x})", gc.msg_type), vec![header, body])
            }
            Err(e) => Node::leaf(format!("GC message: {e}")),
        }
    }
}

fn steam_id(raw: u64) -> String {
    let id = SteamId::from_u64(raw);
    if raw == 0 {
        "0".into()
    } else {
        format!("{raw} ({})", id.steam3())
    }
}

fn message_node(label: String, msg: &DynamicMessage) -> Node {
    let children = msg.fields().map(|(field, value)| value_node(field.name(), &field.kind(), value)).collect();
    Node::branch(label, children)
}

fn value_node(name: &str, kind: &Kind, value: &Value) -> Node {
    match value {
        Value::Message(m) => message_node(name.to_owned(), m),
        Value::List(items) => Node::branch(
            format!("{name} ({})", items.len()),
            items.iter().enumerate().map(|(i, v)| value_node(&format!("[{i}]"), kind, v)).collect(),
        ),
        Value::Map(map) => {
            let value_kind = match kind {
                Kind::Message(entry) => entry.map_entry_value_field().kind(),
                k => k.clone(),
            };
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(k, _)| map_key(k));
            Node::branch(
                format!("{name} ({})", map.len()),
                entries.into_iter().map(|(k, v)| value_node(&map_key(k), &value_kind, v)).collect(),
            )
        }
        Value::EnumNumber(n) => {
            let label = match kind {
                Kind::Enum(e) => match e.get_value(*n) {
                    Some(v) => format!("{name}: {} ({n})", v.name()),
                    None => format!("{name}: {n}"),
                },
                _ => format!("{name}: {n}"),
            };
            Node::leaf(label)
        }
        Value::Bytes(b) => bytes_node(name, b),
        Value::String(s) if s.contains('\n') => {
            Node::branch(format!("{name}: ({} lines)", s.lines().count()), s.lines().map(Node::leaf).collect())
        }
        Value::String(s) => Node::leaf(format!("{name}: {s:?}")),
        Value::U64(v) if name.contains("steamid") || name.contains("steam_id") => {
            Node::leaf(format!("{name}: {}", steam_id(*v)))
        }
        Value::Bool(v) => Node::leaf(format!("{name}: {v}")),
        Value::I32(v) => Node::leaf(format!("{name}: {v}")),
        Value::I64(v) => Node::leaf(format!("{name}: {v}")),
        Value::U32(v) => Node::leaf(format!("{name}: {v}")),
        Value::U64(v) => Node::leaf(format!("{name}: {v}")),
        Value::F32(v) => Node::leaf(format!("{name}: {v}")),
        Value::F64(v) => Node::leaf(format!("{name}: {v}")),
    }
}

fn map_key(key: &MapKey) -> String {
    match key {
        MapKey::Bool(v) => v.to_string(),
        MapKey::I32(v) => v.to_string(),
        MapKey::I64(v) => v.to_string(),
        MapKey::U32(v) => v.to_string(),
        MapKey::U64(v) => v.to_string(),
        MapKey::String(v) => format!("{v:?}"),
    }
}

/// Bytes shown as text, binary KeyValues or hex, whichever fits.
fn bytes_node(name: &str, data: &[u8]) -> Node {
    if data.is_empty() {
        return Node::leaf(format!("{name}: (empty)"));
    }
    if let Ok(text) = std::str::from_utf8(data.strip_suffix(&[0]).unwrap_or(data)) {
        if text.chars().all(|c| !c.is_control() || c.is_whitespace()) {
            if text.trim_start().starts_with('"') {
                if let Ok(kv) = KeyValue::parse_text(text) {
                    return Node::branch(format!("{name}: KeyValues, {} bytes", data.len()), vec![kv_node(&kv)]);
                }
            }
            if text.contains('\n') {
                return Node::branch(
                    format!("{name}: text, {} bytes", data.len()),
                    text.lines().map(Node::leaf).collect(),
                );
            }
            return Node::leaf(format!("{name}: {text:?}"));
        }
    }
    if let Ok((kv, used)) = KeyValue::parse_binary_prefix(data) {
        if used == data.len() {
            return Node::branch(format!("{name}: binary KeyValues, {} bytes", data.len()), vec![kv_node(&kv)]);
        }
    }
    Node::leaf(format!("{name}: {} bytes: {}", data.len(), hex_preview(data)))
}

fn kv_node(kv: &KeyValue) -> Node {
    if kv.children().is_empty() {
        let value = kv.to_value_string().unwrap_or_default();
        return Node::leaf(format!("{:?}: {value:?}", kv.name));
    }
    Node::branch(format!("{:?}", kv.name), kv.children().iter().map(kv_node).collect())
}

fn payload_node(data: &[u8]) -> Node {
    if data.is_empty() {
        return Node::leaf("Payload: (empty)");
    }
    bytes_node("Payload", data)
}

/// Shows protobuf fields without a type (NetHookAnalyzer2's
/// `ProtoBufFieldReader`).
pub(crate) fn raw_protobuf(label: &str, data: &[u8]) -> Node {
    let mut children = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let Some((key, n)) = varint(rest) else { break };
        rest = &rest[n..];
        let (field, wire) = (key >> 3, key & 7);
        let value = match wire {
            0 => varint(rest).map(|(v, n)| (format!("{v}"), n)),
            1 if rest.len() >= 8 => Some((u64::from_le_bytes(rest[..8].try_into().unwrap()).to_string(), 8)),
            5 if rest.len() >= 4 => Some((u32::from_le_bytes(rest[..4].try_into().unwrap()).to_string(), 4)),
            2 => varint(rest).and_then(|(len, n)| {
                let end = n.checked_add(usize::try_from(len).ok()?)?;
                let bytes = rest.get(n..end)?;
                Some((bytes_node("", bytes).label.trim_start_matches(": ").to_owned(), end))
            }),
            _ => None,
        };
        let Some((value, used)) = value else {
            children.push(Node::leaf(format!("field {field}: unreadable wire type {wire}")));
            break;
        };
        children.push(Node::leaf(format!("field {field}: {value}")));
        rest = &rest[used..];
    }
    if !rest.is_empty() && !children.is_empty() {
        children.push(Node::leaf(format!("{} unread bytes: {}", rest.len(), hex_preview(rest))));
    }
    if children.is_empty() && !data.is_empty() {
        children.push(Node::leaf(format!("{} bytes: {}", data.len(), hex_preview(data))));
    }
    Node::branch(label, children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_body_types() {
        let a = Analyzer::new();
        assert_eq!(a.body_type(EMsg::ClientLogon).unwrap().name(), "CMsgClientLogon");
        assert_eq!(a.body_type(EMsg::ClientToGC).unwrap().name(), "CMsgGCClient");
        assert_eq!(
            a.body_type(EMsg::ClientPICSProductInfoResponse).unwrap().name(),
            "CMsgClientPICSProductInfoResponse"
        );
        assert_eq!(
            a.service_method_type("Player.GetGameBadgeLevels#1", false).unwrap().name(),
            "CPlayer_GetGameBadgeLevels_Request"
        );
        assert_eq!(
            a.service_method_type("Player.GetGameBadgeLevels#1", true).unwrap().name(),
            "CPlayer_GetGameBadgeLevels_Response"
        );
    }

    #[test]
    fn raw_fields() {
        // field 1 varint 150, field 2 "hi"
        let node = raw_protobuf("x", &[0x08, 0x96, 0x01, 0x12, 0x02, b'h', b'i']);
        let labels: Vec<_> = node.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["field 1: 150", "field 2: \"hi\""]);
    }
}
