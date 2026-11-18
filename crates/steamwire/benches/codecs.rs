//! Throughput of the CPU-bound parts: chunk processing (AES + LZMA +
//! Adler-32), VZip, manifests, KeyValues, packets and channel encryption.
//!
//! ```sh
//! cargo bench -p steamwire --bench codecs
//! ```

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use steamwire::cdn::{chunk, ChunkData, DepotManifest};
use steamwire::kv::KeyValue;
use steamwire::messages::{Packet, ProtoMsg};
use steamwire::net::encryption::ChannelCipher;
use steamwire::protos::steam::CMsgClientHeartBeat;
use steamwire::util::compression::{vzip_compress, vzip_decompress};
use steamwire::EMsg;

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Text that compresses like game data does: words from a small set.
fn words(len: usize) -> Vec<u8> {
    const WORDS: [&[u8]; 8] = [b"steam ", b"depot ", b"chunk ", b"the ", b"manifest\n", b"0123456789", b"lzma ", b"a "];
    let mut x = 42u32;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.extend_from_slice(WORDS[(x >> 29) as usize]);
    }
    out.truncate(len);
    out
}

fn chunks(c: &mut Criterion) {
    // A real 111 KiB VZip chunk of depot 1004, with its key.
    const KEY: [u8; 32] = [
        0xF2, 0x1F, 0xAA, 0x4F, 0x29, 0xEC, 0xEE, 0x9D, 0x19, 0x2F, 0xC1, 0x78, 0x4D, 0x17, 0x01, 0x37, 0x6E, 0xB8,
        0x7E, 0x13, 0x52, 0x18, 0x24, 0xCE, 0xAF, 0x01, 0x16, 0xB1, 0x0C, 0x55, 0x8A, 0x81,
    ];
    let data = fixture("cdn/depot_1004_chunk_b4e70afde4693284936da1a67758c897a492e1fc.bin");
    let info = ChunkData {
        chunk_id: hex_id("B4E70AFDE4693284936DA1A67758C897A492E1FC"),
        checksum: 1759764290,
        compressed_length: 33536,
        uncompressed_length: 113816,
        ..Default::default()
    };
    let mut out = vec![0u8; info.uncompressed_length as usize];
    let mut group = c.benchmark_group("chunk");
    group.throughput(Throughput::Bytes(info.uncompressed_length as u64));
    group.bench_function("process (AES + VZip + Adler-32)", |b| {
        b.iter(|| chunk::process(&info, black_box(&data), &mut out, &KEY).unwrap())
    });
    group.finish();
}

fn hex_id(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn vzip(c: &mut Criterion) {
    let mut group = c.benchmark_group("vzip");
    for size in [64 << 10, 1 << 20] {
        let input = words(size);
        let compressed = vzip_compress(&input);
        let mut out = vec![0u8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("decompress", size), &compressed, |b, data| {
            b.iter(|| vzip_decompress(black_box(data), &mut out, true).unwrap())
        });
        group.sample_size(10);
        group.bench_with_input(BenchmarkId::new("compress", size), &input, |b, data| {
            b.iter(|| vzip_compress(black_box(data)))
        });
    }
    group.finish();
}

fn manifests(c: &mut Criterion) {
    let data = fixture("cdn/depot_440_1118032470228587934_decrypted.manifest");
    c.bench_function("manifest deserialize", |b| b.iter(|| DepotManifest::deserialize(black_box(&data)).unwrap()));
}

fn key_values(c: &mut Criterion) {
    let depots: Vec<KeyValue> = (0..200)
        .map(|i| {
            KeyValue::new_object(
                i.to_string(),
                vec![
                    KeyValue::new_string("name", format!("depot {i}")),
                    KeyValue::new_object("manifests", vec![KeyValue::new_string("public", "1234567890123456789")]),
                ],
            )
        })
        .collect();
    let root = KeyValue::new_object("appinfo", vec![KeyValue::new_object("depots", depots)]);
    let text = root.to_text();
    let binary = root.to_binary();
    let mut group = c.benchmark_group("key values");
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("parse text", |b| b.iter(|| KeyValue::parse_text(black_box(&text)).unwrap()));
    group.throughput(Throughput::Bytes(binary.len() as u64));
    group.bench_function("parse binary", |b| b.iter(|| KeyValue::parse_binary(black_box(&binary)).unwrap()));
    group.finish();
}

fn packets(c: &mut Criterion) {
    let encoded = ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat::default()).encode();
    c.bench_function("packet parse", |b| b.iter(|| Packet::parse(black_box(encoded.clone()).into()).unwrap()));

    let cipher = ChannelCipher::new(&[7; 32]);
    let message = words(1400);
    let encrypted = cipher.encrypt(&message);
    let mut group = c.benchmark_group("channel encryption");
    group.throughput(Throughput::Bytes(message.len() as u64));
    group.bench_function("encrypt", |b| b.iter(|| cipher.encrypt(black_box(&message))));
    group.bench_function("decrypt", |b| b.iter(|| cipher.decrypt(black_box(&encrypted)).unwrap()));
    group.finish();
}

criterion_group!(benches, chunks, vzip, manifests, key_values, packets);
criterion_main!(benches);
