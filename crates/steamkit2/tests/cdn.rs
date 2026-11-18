//! Depot chunks and manifests against fixtures (from steamkit-rs, which took
//! them from SteamKit2's tests).

use std::path::MAIN_SEPARATOR_STR as SEP;

use sha1::{Digest, Sha1};
use steamkit2::cdn::{chunk, ChunkData, DepotManifest};
use steamkit2::util::compression::vzip_decompress;
use steamkit2::util::crypto::symmetric_decrypt;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/cdn/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn sha1(data: &[u8]) -> Vec<u8> {
    Sha1::digest(data).to_vec()
}

fn chunk_info(id: &str, checksum: u32, compressed: u32, uncompressed: u32) -> ChunkData {
    ChunkData {
        chunk_id: hex::decode(id).unwrap(),
        checksum,
        offset: 0,
        compressed_length: compressed,
        uncompressed_length: uncompressed,
    }
}

const DEPOT440_KEY: [u8; 32] = [
    0x44, 0xCE, 0x5C, 0x52, 0x97, 0xA4, 0x15, 0xA1, 0xA6, 0xF6, 0x9C, 0x85, 0x60, 0x37, 0xA5, 0xA2, 0xFD, 0xD8, 0x2C,
    0xD4, 0x74, 0xFA, 0x65, 0x9E, 0xDF, 0xB4, 0xD5, 0x9B, 0x2A, 0xBC, 0x55, 0xFC,
];
const DEPOT232250_KEY: [u8; 32] = [
    0xE5, 0xF6, 0xAE, 0xD5, 0x5E, 0x9E, 0xCE, 0x42, 0x9E, 0x56, 0xB8, 0x13, 0xFB, 0xF6, 0xBF, 0xE9, 0x24, 0xF3, 0xCF,
    0x72, 0x97, 0x2F, 0xDB, 0xD0, 0x57, 0x1F, 0xFC, 0xAD, 0x9F, 0x2F, 0x7D, 0xAA,
];
const DEPOT3441461_KEY: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32,
];
const DEPOT4_KEY: [u8; 32] = [
    0x57, 0xEF, 0x98, 0xDE, 0x53, 0x88, 0xA8, 0x4C, 0xBD, 0x6B, 0xA9, 0x4D, 0xCD, 0xA3, 0x6F, 0x8B, 0xB7, 0x49, 0x17,
    0x7D, 0xAA, 0xBF, 0x35, 0x21, 0x2F, 0xEF, 0xCC, 0x09, 0x83, 0x33, 0xE4, 0xD5,
];
const DEPOT1004_KEY: [u8; 32] = [
    0xF2, 0x1F, 0xAA, 0x4F, 0x29, 0xEC, 0xEE, 0x9D, 0x19, 0x2F, 0xC1, 0x78, 0x4D, 0x17, 0x01, 0x37, 0x6E, 0xB8, 0x7E,
    0x13, 0x52, 0x18, 0x24, 0xCE, 0xAF, 0x01, 0x16, 0xB1, 0x0C, 0x55, 0x8A, 0x81,
];

fn depot4_chunk() -> ChunkData {
    chunk_info("DC036A660987D8F705936D4443FEDB5E1D2E5D9B", 649235822, 25072, 68887)
}

fn depot1004_chunk() -> ChunkData {
    chunk_info("B4E70AFDE4693284936DA1A67758C897A492E1FC", 1759764290, 33536, 113816)
}

fn check_chunk(file: &str, info: &ChunkData, key: &[u8], expected_sha: &str) {
    let raw = data(file);
    assert_eq!(raw.len(), info.compressed_length as usize);
    let out = chunk::process_to_vec(info, &raw, key).unwrap();
    assert_eq!(out.len(), info.uncompressed_length as usize);
    assert_eq!(hex::encode_upper(sha1(&out)), expected_sha);
}

#[test]
fn chunk_pkzip() {
    let info =
        ChunkData { checksum: 2130218374, compressed_length: 320, uncompressed_length: 544, ..Default::default() };
    check_chunk(
        "depot_440_chunk_bac8e2657470b2eb70d6ddcd6c07004be8738697.bin",
        &info,
        &DEPOT440_KEY,
        "BAC8E2657470B2EB70D6DDCD6C07004BE8738697",
    );
}

#[test]
fn chunk_vzip() {
    let info =
        ChunkData { checksum: 2894626744, compressed_length: 304, uncompressed_length: 798, ..Default::default() };
    check_chunk(
        "depot_232250_chunk_7b8567d9b3c09295cdbf4978c32b348d8e76c750.bin",
        &info,
        &DEPOT232250_KEY,
        "7B8567D9B3C09295CDBF4978C32B348D8E76C750",
    );
}

#[test]
fn chunk_vzstd() {
    let info =
        ChunkData { checksum: 3753325726, compressed_length: 176, uncompressed_length: 156, ..Default::default() };
    check_chunk(
        "depot_3441461_chunk_9e72678e305540630a665b93e1463bc3983eb55a.bin",
        &info,
        &DEPOT3441461_KEY,
        "9E72678E305540630A665B93E1463BC3983EB55A",
    );
}

#[test]
fn larger_chunks() {
    for (file, info, key) in [
        ("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin", depot4_chunk(), &DEPOT4_KEY),
        ("depot_1004_chunk_b4e70afde4693284936da1a67758c897a492e1fc.bin", depot1004_chunk(), &DEPOT1004_KEY),
    ] {
        let out = chunk::process_to_vec(&info, &data(file), key).unwrap();
        assert_eq!(sha1(&out), info.chunk_id, "{file}");
    }
}

#[test]
fn corrupt_chunks_fail() {
    for (file, info, key) in [
        ("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin", depot4_chunk(), &DEPOT4_KEY),
        ("depot_1004_chunk_b4e70afde4693284936da1a67758c897a492e1fc.bin", depot1004_chunk(), &DEPOT1004_KEY),
    ] {
        let mut raw = data(file);
        let middle = raw.len() / 2;
        raw[middle] ^= 0x55;
        assert!(chunk::process_to_vec(&info, &raw, key).is_err(), "{file}");
    }
}

#[test]
fn wrong_key_fails() {
    let raw = data("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin");
    assert!(chunk::process_to_vec(&depot4_chunk(), &raw, &DEPOT1004_KEY).is_err());
}

#[test]
fn vzip_with_small_dictionary() {
    let mut payload =
        symmetric_decrypt(&data("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin"), &DEPOT4_KEY).unwrap();
    payload[8..12].copy_from_slice(&(1u32 << 12).to_le_bytes());
    let info = depot4_chunk();
    let mut out = vec![0u8; info.uncompressed_length as usize];
    let n = vzip_decompress(&payload, &mut out, true).unwrap();
    assert_eq!(sha1(&out[..n]), info.chunk_id);
}

#[test]
fn vzip_truncated_output() {
    let payload =
        symmetric_decrypt(&data("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin"), &DEPOT4_KEY).unwrap();
    let full_len = depot4_chunk().uncompressed_length as usize;
    let mut full = vec![0u8; full_len];
    vzip_decompress(&payload, &mut full, true).unwrap();
    for len in [full_len - 64, full_len - 1] {
        let mut p = payload.clone();
        let at = p.len() - 6;
        p[at..at + 4].copy_from_slice(&(len as i32).to_le_bytes());
        let mut out = vec![0u8; len];
        let n = vzip_decompress(&p, &mut out, false).unwrap();
        assert_eq!(&out[..n], &full[..len]);
    }
}

fn assert_decrypted_manifest(manifest: &DepotManifest) {
    assert!(!manifest.filenames_encrypted);
    assert_eq!(manifest.depot_id, 440);
    assert_eq!(manifest.manifest_gid, 1118032470228587934);
    assert_eq!(manifest.total_uncompressed_size, 825745);
    assert_eq!(manifest.total_compressed_size, 43168);
    assert_eq!(manifest.creation_time, 1_366_231_164);
    let names: Vec<&str> = manifest.files.iter().map(|f| f.file_name.as_str()).collect();
    let expected = [
        format!("bin{SEP}dxsupport.cfg"),
        format!("bin{SEP}dxsupport.csv"),
        format!("bin{SEP}dxsupport_episodic.cfg"),
        format!("bin{SEP}dxsupport_sp.cfg"),
        format!("bin{SEP}vidcfg.bin"),
        format!("hl2{SEP}media{SEP}startupvids.txt"),
        format!("tf{SEP}media{SEP}startupvids.txt"),
    ];
    assert_eq!(names, expected);
    assert_eq!(manifest.files[0].flags.0, 0);
    assert_eq!(manifest.files[0].total_size, 398709);
    assert_eq!(manifest.files[2].file_hash, hex::decode("BAC8E2657470B2EB70D6DDCD6C07004BE8738697").unwrap());
    for file in &manifest.files {
        assert_eq!(sha1(file.file_name.replace('/', "\\").as_bytes()), file.file_name_hash);
        assert_eq!(file.chunks.len(), 1);
    }
    let chunk = &manifest.files[6].chunks[0];
    assert_eq!(chunk.checksum, 963249608);
    assert_eq!(chunk.compressed_length, 144);
    assert_eq!(chunk.uncompressed_length, 17);
    assert_eq!(chunk.chunk_id, hex::decode("94020BDE145A521EDEC9A9424E7A90FD042481E9").unwrap());
}

#[test]
fn manifest_v4_binary() {
    let mut manifest = DepotManifest::deserialize(&data("depot_440_1118032470228587934_v4.manifest")).unwrap();
    assert!(manifest.filenames_encrypted);
    assert_eq!(manifest.encrypted_crc, 1195249848);
    manifest.decrypt_filenames(&DEPOT440_KEY).unwrap();
    assert_decrypted_manifest(&manifest);
}

#[test]
fn manifest_protobuf_encrypted() {
    let mut manifest = DepotManifest::deserialize(&data("depot_440_1118032470228587934.manifest")).unwrap();
    assert!(manifest.filenames_encrypted);
    assert_eq!(manifest.encrypted_crc, 1606273976);
    manifest.decrypt_filenames(&DEPOT440_KEY).unwrap();
    assert_decrypted_manifest(&manifest);
}

#[test]
fn manifest_protobuf_decrypted() {
    let manifest = DepotManifest::deserialize(&data("depot_440_1118032470228587934_decrypted.manifest")).unwrap();
    assert_decrypted_manifest(&manifest);
}

#[test]
fn manifest_round_trip_decrypted_is_byte_identical() {
    let original = data("depot_440_1118032470228587934_decrypted.manifest");
    let manifest = DepotManifest::deserialize(&original).unwrap();
    assert_eq!(manifest.serialize(), original);
}

#[test]
fn manifest_file_round_trip() {
    let manifest = DepotManifest::deserialize(&data("depot_440_1118032470228587934_decrypted.manifest")).unwrap();
    let path = std::env::temp_dir().join(format!("steamkit-manifest-{}.bin", std::process::id()));
    manifest.save_to_file(&path).unwrap();
    assert_eq!(DepotManifest::load_from_file(&path).unwrap(), manifest);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn manifest_round_trip_encrypted_matches_outside_signature() {
    const SIGNATURE: [u8; 4] = [0x17, 0xB8, 0x81, 0x1B];
    let find = |h: &[u8]| h.windows(4).position(|w| w == SIGNATURE).unwrap();
    let original = data("depot_440_1118032470228587934.manifest");
    let manifest = DepotManifest::deserialize(&original).unwrap();
    let ours = manifest.serialize();
    let (a, b) = (find(&original), find(&ours));
    assert_eq!(&original[..a], &ours[..b]);
    let signature_len = u32::from_le_bytes(original[a + 4..a + 8].try_into().unwrap()) as usize;
    assert_eq!(signature_len, 131);
    assert_eq!(&ours[b + 4..b + 8], &[0, 0, 0, 0]);
    assert_eq!(&original[a + 8 + signature_len..], &ours[b + 8..]);
    assert_eq!(DepotManifest::deserialize(&ours).unwrap(), manifest);
}

/// A VZip delta made by an independent LZMA encoder (steamkit-rs's port of
/// the LZMA SDK, with the source as training data), following SteamKit2's
/// `LzmaFacts`: the depot 4 chunk, with 1000 bytes inserted at 20000 and
/// the last byte flipped.
#[test]
fn vzip_delta_against_source_chunk() {
    let contents = chunk::process_to_vec(
        &depot4_chunk(),
        &data("depot_4_chunk_dc036a660987d8f705936d4443fedb5e1d2e5d9b.bin"),
        &DEPOT4_KEY,
    )
    .unwrap();
    let mut len = contents.len();
    while len > 0 && contents[len - 1] >= 32 {
        len -= 1;
    }
    let source = &contents[..len];
    let mut target = source[..20000].to_vec();
    let mut x = 1950u32;
    for _ in 0..1000 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        target.push((x >> 16) as u8);
    }
    target.extend_from_slice(&source[20000..]);
    *target.last_mut().unwrap() ^= 0xFF;

    let delta = data("depot_4_delta.vzd");
    // Most of the target comes from the source.
    assert!(delta.len() < target.len() / 10, "delta is {} bytes", delta.len());
    assert_eq!(steamkit2::util::compression::vzip_delta_decompress(&delta, source).unwrap(), target);
    // Matches copy from the source, so a different source fails the CRC.
    let mut wrong = source.to_vec();
    wrong[30000..31000].fill(0);
    assert!(steamkit2::util::compression::vzip_delta_decompress(&delta, &wrong).is_err());
}
