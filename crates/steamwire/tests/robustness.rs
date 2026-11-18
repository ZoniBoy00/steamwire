//! Parsers of data from the network or the CDN must reject bad input with an
//! error, never panic. Each target gets valid samples, random bytes and
//! mutations of the samples (bit flips, truncation, inserted and overwritten
//! bytes, large length fields). `fuzz/` has the same targets for
//! cargo-fuzz, which searches much further.

use std::panic::{catch_unwind, AssertUnwindSafe};

use steamwire::cdn::{chunk, ChunkData, DepotManifest};
use steamwire::kv::KeyValue;
use steamwire::messages::gc::GcPacket;
use steamwire::messages::{split_multi, Packet, ProtoMsg};
use steamwire::net::udp::UdpPacket;
use steamwire::protos::steam::{CMsgClientHeartBeat, CMsgMulti};
use steamwire::util::compression::{vzip_compress, vzip_decompress, vzip_delta_decompress, zip_decompress};
use steamwire::util::lzma;
use steamwire::{EMsg, SteamId};

/// Inputs per target; `STEAMWIRE_ROUNDS` raises it for a longer search.
fn rounds() -> usize {
    std::env::var("STEAMWIRE_ROUNDS").ok().and_then(|r| r.parse().ok()).unwrap_or(3000)
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

fn mutate(rng: &mut Lcg, sample: &[u8]) -> Vec<u8> {
    let mut data = sample.to_vec();
    for _ in 0..1 + rng.below(4) {
        match rng.below(7) {
            0 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] ^= 1 << rng.below(8);
            }
            1 if !data.is_empty() => {
                let i = rng.below(data.len());
                data[i] = rng.next() as u8;
            }
            2 => data.truncate(rng.below(data.len() + 1)),
            3 => {
                let i = rng.below(data.len() + 1);
                data.insert(i, rng.next() as u8);
            }
            4 if data.len() >= 4 => {
                // A length or size field set to something huge.
                let i = rng.below(data.len() - 3);
                let value: u32 = [u32::MAX, 0x7fff_ffff, 0x8000_0000, 1 << 20][rng.below(4)];
                data[i..i + 4].copy_from_slice(&value.to_le_bytes());
            }
            5 if !data.is_empty() => {
                let i = rng.below(data.len());
                data.drain(i..(i + rng.below(16)).min(data.len()));
            }
            _ => data.extend((0..rng.below(16)).map(|_| rng.next() as u8)),
        }
    }
    data
}

/// Feeds `target` the samples, random data and mutations.
fn check(name: &str, samples: &[Vec<u8>], target: impl Fn(&[u8])) {
    let mut rng = Lcg(name.bytes().fold(0, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)));
    let mut inputs: Vec<Vec<u8>> = samples.to_vec();
    inputs.push(Vec::new());
    for round in 0..rounds() {
        let input = if samples.is_empty() || round % 10 == 0 {
            (0..rng.below(256)).map(|_| rng.next() as u8).collect()
        } else {
            let sample = rng.below(samples.len());
            mutate(&mut rng, &samples[sample])
        };
        inputs.push(input);
    }
    for input in inputs {
        if catch_unwind(AssertUnwindSafe(|| target(&input))).is_err() {
            panic!("{name} panicked on {} bytes: {}", input.len(), hex(&input));
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.iter().take(512).map(|b| format!("{b:02x}")).collect()
}

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn packets() {
    let heartbeat = ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat::default()).encode();
    let multi = ProtoMsg::new(
        EMsg::Multi,
        CMsgMulti {
            size_unzipped: Some(0),
            message_body: Some([&(heartbeat.len() as u32).to_le_bytes()[..], &heartbeat].concat()),
        },
    )
    .encode();
    let pics = fixture("pics/002_in_8904_k_EMsgClientPICSProductInfoResponse_app480.bin");
    check("packet", &[heartbeat, multi, pics], |data| {
        if let Ok(packet) = Packet::parse(data.to_vec().into()) {
            let _ = split_multi(&packet);
            let _ = packet.encode();
            let _ = steamwire::messages::generated::describe_body(packet.emsg, &packet.body);
        }
    });
}

#[test]
fn gc_packets() {
    let samples = [vec![0u8; 18], [&[0xff; 8][..], &[0u8; 16]].concat()];
    check("gc", &samples, |data| {
        let _ = GcPacket::parse(0x8000_0000 | 4004, data.to_vec().into());
        let _ = GcPacket::parse(4004, data.to_vec().into());
    });
}

#[test]
fn udp_packets() {
    check("udp", &[], |data| {
        let _ = UdpPacket::parse(data);
    });
}

#[test]
fn key_values() {
    let kv = KeyValue::new_object(
        "root",
        vec![
            KeyValue::new_string("name", "Spacewar"),
            KeyValue::new_object("depots", vec![KeyValue::new_string("481", "x")]),
        ],
    );
    let text = kv.to_text().into_bytes();
    let binary = kv.to_binary();
    check("kv text", &[text, b"\"a\" { \"b\" \"c\" #include \"x\" }".to_vec()], |data| {
        if let Ok(text) = std::str::from_utf8(data) {
            let _ = KeyValue::parse_text(text);
        }
    });
    check("kv binary", &[binary], |data| {
        let _ = KeyValue::parse_binary(data);
        let _ = KeyValue::parse_binary_all(data);
    });
}

#[test]
fn manifests() {
    let samples = [
        fixture("cdn/depot_440_1118032470228587934_decrypted.manifest"),
        fixture("cdn/depot_440_1118032470228587934_v4.manifest"),
    ];
    check("manifest", &samples, |data| {
        if let Ok(mut manifest) = DepotManifest::deserialize(data) {
            let _ = manifest.decrypt_filenames(&[7; 32]);
            let _ = manifest.serialize();
        }
    });
    check("zipped manifest", &[fixture("cdn/depot_440_1118032470228587934.manifest")], |data| {
        let _ = zip_decompress(data, true);
    });
}

#[test]
fn compression() {
    let samples = [vzip_compress(b"hello hello hello hello"), vzip_compress(&[0u8; 300])];
    check("vzip", &samples, |data| {
        let mut out = vec![0u8; 4096];
        let _ = vzip_decompress(data, &mut out, true);
        let _ = vzip_delta_decompress(data, b"source data");
    });
    check("vzip delta", &[fixture("cdn/depot_4_delta.vzd")], |data| {
        let _ = vzip_delta_decompress(data, &[1u8; 2048]);
    });
    let raw = vzip_compress(b"lzma lzma lzma lzma");
    let (props, stream) = (raw[7..12].to_vec(), raw[12..raw.len() - 10].to_vec());
    check("lzma", &[[props, stream].concat()], |data| {
        if data.len() >= 5 {
            let mut out = vec![0u8; 256];
            let _ = lzma::decode(&data[..5], &data[5..], &mut out);
        }
    });
}

#[test]
fn chunks() {
    let info = ChunkData { uncompressed_length: 4096, ..Default::default() };
    check("chunk", &[fixture("cdn/depot_440_chunk_bac8e2657470b2eb70d6ddcd6c07004be8738697.bin")], |data| {
        let mut out = vec![0u8; 4096];
        let _ = chunk::process(&info, data, &mut out, &[3; 32]);
    });
}

#[test]
fn steam_ids() {
    let samples =
        ["[U:1:46143802]", "STEAM_0:0:23071901", "[g:1:4]", "[L:1:4028313011]"].map(|s| s.as_bytes().to_vec());
    check("steam id", &samples, |data| {
        if let Ok(text) = std::str::from_utf8(data) {
            let _ = SteamId::parse_steam2(text);
            let _ = SteamId::parse_steam3(text);
            let _ = text.parse::<SteamId>();
        }
    });
}
