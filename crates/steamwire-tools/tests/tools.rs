use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use bytes::Bytes;
use prost::Message;
use prost_types::FileDescriptorProto;
use steamwire::messages::{Header, MsgHdr, Packet};
use steamwire::protos::steam::{CMsgClientHeartBeat, CMsgClientLogon, CMsgMulti, CPlayerGetGameBadgeLevelsRequest};
use steamwire::{EMsg, JobId, NetHookListener, NetworkListener, ProtoMsg};
use steamwire_tools::nethook::{Analyzer, Direction, Dump};
use steamwire_tools::protodump;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("steamwire-tools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a dump with the listener the client uses, then reads it back.
#[test]
fn nethook_dump_round_trip() {
    let dir = temp_dir("nethook");
    let listener = NetHookListener::with_directory(&dir).unwrap();

    let plain = Packet {
        emsg: EMsg::ChannelEncryptResult,
        header: Header::Plain(MsgHdr { target_job: JobId::NONE, source_job: JobId::NONE }),
        body: Bytes::from_static(&[1, 0, 0, 0]),
    };
    listener.on_incoming(plain.emsg, &plain.encode());

    let logon = ProtoMsg::new(
        EMsg::ClientLogon,
        CMsgClientLogon { account_name: Some("someone".into()), protocol_version: Some(65581), ..Default::default() },
    );
    listener.on_outgoing(EMsg::ClientLogon, &logon.encode());

    let mut service =
        ProtoMsg::new(EMsg::ServiceMethodCallFromClient, CPlayerGetGameBadgeLevelsRequest { appid: Some(440) });
    service.header.target_job_name = Some("Player.GetGameBadgeLevels#1".into());
    listener.on_outgoing(EMsg::ServiceMethodCallFromClient, &service.encode());

    let heartbeat = ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat::default()).encode();
    let mut raw = Vec::new();
    for _ in 0..2 {
        raw.extend_from_slice(&(heartbeat.len() as u32).to_le_bytes());
        raw.extend_from_slice(&heartbeat);
    }
    let multi = ProtoMsg::new(EMsg::Multi, CMsgMulti { size_unzipped: Some(0), message_body: Some(raw) });
    listener.on_incoming(EMsg::Multi, &multi.encode());

    let dump = Dump::load(&dir).unwrap();
    let summary: Vec<_> = dump.items.iter().map(|i| (i.sequence, i.direction, i.emsg)).collect();
    assert_eq!(
        summary,
        [
            (1, Direction::In, EMsg::ChannelEncryptResult),
            (2, Direction::Out, EMsg::ClientLogon),
            (3, Direction::Out, EMsg::ServiceMethodCallFromClient),
            (4, Direction::In, EMsg::Multi),
        ]
    );

    let analyzer = Analyzer::new();
    let trees: Vec<_> = dump.items.iter().map(|i| analyzer.message(&i.read().unwrap())).collect();

    assert!(trees[0].find("Body: MsgChannelEncryptResult").is_some(), "{}", trees[0]);
    assert!(trees[0].find("result: EResult::OK").is_some(), "{}", trees[0]);

    assert!(trees[1].find("Body: CMsgClientLogon").is_some(), "{}", trees[1]);
    assert!(trees[1].find("account_name: \"someone\"").is_some(), "{}", trees[1]);

    let body = trees[2].find("Body: CPlayer_GetGameBadgeLevels_Request").expect("service body");
    assert_eq!(body.children[0].label, "appid: 440");

    let messages = trees[3].find("Messages (2)").expect("unpacked multi");
    assert!(messages.children.iter().all(|m| m.label.starts_with("ClientHeartBeat")), "{}", trees[3]);

    std::fs::remove_dir_all(&dir).unwrap();
}

/// The files of a serialized `FileDescriptorSet`, as serialized (decoding
/// with prost would drop custom options).
fn raw_files(set: &[u8]) -> Vec<(String, Vec<u8>)> {
    fn varint(data: &[u8], pos: &mut usize) -> usize {
        let mut value = 0usize;
        for shift in (0..).step_by(7) {
            let b = data[*pos];
            *pos += 1;
            value |= ((b & 0x7F) as usize) << shift;
            if b & 0x80 == 0 {
                break;
            }
        }
        value
    }
    let mut files = Vec::new();
    let mut pos = 0;
    while pos < set.len() {
        assert_eq!(varint(set, &mut pos), (1 << 3) | 2, "FileDescriptorSet.file");
        let len = varint(set, &mut pos);
        let raw = set[pos..pos + len].to_vec();
        pos += len;
        files.push((FileDescriptorProto::decode(&raw[..]).unwrap().name().to_owned(), raw));
    }
    files
}

fn protoc(include: &[&Path], files: &[&str], out: &Path) -> Vec<(String, Vec<u8>)> {
    let protoc = protoc_bin_vendored::protoc_bin_path().unwrap();
    let mut cmd = Command::new(protoc);
    for dir in include {
        cmd.arg(format!("-I{}", dir.display()));
    }
    cmd.arg(format!("-I{}", protoc_bin_vendored::include_path().unwrap().display()));
    cmd.arg("--include_imports").arg(format!("--descriptor_set_out={}", out.display()));
    cmd.args(files);
    let status = cmd.output().unwrap();
    assert!(status.status.success(), "protoc failed: {}", String::from_utf8_lossy(&status.stderr));
    raw_files(&std::fs::read(out).unwrap())
}

/// Dumps descriptors hidden in a fake binary, writes them as `.proto`,
/// recompiles the output with protoc and checks nothing changed.
#[test]
fn protodump_round_trip() {
    let protos = Path::new(env!("CARGO_MANIFEST_DIR")).join("../steamwire-protos/proto/steam");
    let dir = temp_dir("protodump");
    // Custom options, services, enums, nested types and imports.
    let files = [
        "steammessages_base.proto",
        "steammessages_unified_base.steamclient.proto",
        "steammessages_player.steamclient.proto",
        "steammessages_clientserver_login.proto",
        "enums.proto",
    ];
    let original = protoc(&[&protos], &files, &dir.join("original.pb"));

    // A "binary": descriptors with junk around them, some of it looking
    // like the start of a descriptor.
    let mut binary = b"MZ\x90\x00junk\x0a\x05x.pro".to_vec();
    for (_, raw) in &original {
        binary.extend_from_slice(&[0x0A, 0x02, 0xFF, 0xFF, 0, 0, 0]);
        binary.extend_from_slice(raw);
        binary.extend_from_slice(b"\0\0padding\0");
    }
    let found = protodump::scan(&binary);
    let names: Vec<_> = found.iter().map(|f| f.proto.name().to_owned()).collect();
    let expected: Vec<_> = original.iter().map(|(name, _)| name.clone()).collect();
    assert_eq!(names, expected);

    let collected = protodump::build_pool(found);
    assert!(collected.failed.is_empty(), "{:?}", collected.failed);
    let out = dir.join("out");
    for name in &collected.files {
        let path = out.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = protodump::render_file(&collected.pool.get_file_by_name(name).unwrap());
        std::fs::File::create(&path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    let rebuilt = protoc(&[&out], &files, &dir.join("rebuilt.pb"));
    // protoc writes no source info here, so the bytes must match exactly,
    // custom options included.
    let user_files = |set: Vec<(String, Vec<u8>)>| -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = set.into_iter().filter(|(name, _)| !name.starts_with("google/")).collect();
        files.sort();
        files
    };
    let (before, after) = (user_files(original), user_files(rebuilt));
    assert_eq!(before.len(), files.len());
    assert_eq!(after.len(), files.len());
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        if a != b {
            let (a, b) = (FileDescriptorProto::decode(&a[..]).unwrap(), FileDescriptorProto::decode(&b[..]).unwrap());
            panic!("{name} changed in the round trip (decoded equal: {})", a == b);
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Real `ClientPICSProductInfoResponse` captures from steamwire's fixtures.
#[test]
fn nethook_pics_captures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../steamwire/tests/data/pics");
    let dump = Dump::load(&dir).unwrap();
    assert_eq!(dump.items.len(), 3);
    let analyzer = Analyzer::new();
    let tree = analyzer.message(&dump.items[1].read().unwrap());
    assert!(tree.find("Body: CMsgClientPICSProductInfoResponse").is_some(), "{tree}");
    // The app's buffer is text KeyValues, shown as a tree.
    let buffer = tree.find("buffer: KeyValues").expect("parsed buffer");
    assert!(buffer.find("\"name\": \"Spacewar\"").is_some(), "{buffer}");
}
