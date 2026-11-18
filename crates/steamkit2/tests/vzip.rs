//! `vzip_compress` against SteamKit2's `VZipUtil.Compress`. The fixtures in
//! `data/vzip/` were made by running SteamKit2's own `VZipUtil.Compress` and
//! LZMA SDK sources (.NET 8) on the inputs built below.

use steamkit2::util::compression::{vzip_compress, vzip_decompress};

/// The C# harness's LCG.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }
}

fn random(len: usize, seed: u32) -> Vec<u8> {
    let mut lcg = Lcg(seed);
    (0..len).map(|_| (lcg.next() >> 24) as u8).collect()
}

fn words(len: usize, seed: u32) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "steam",
        "kit",
        "depot",
        "manifest",
        "chunk",
        "the",
        "a",
        "of",
        "valve",
        "lzma",
        "\n",
        " ",
        "  ",
        "0123456789",
        "SteamKit2",
        "rust",
    ];
    let mut lcg = Lcg(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        out.extend_from_slice(WORDS[((lcg.next() >> 16) % 16) as usize].as_bytes());
    }
    out.truncate(len);
    out
}

fn check(name: &str, input: &[u8]) {
    let path = format!("{}/tests/data/vzip/{name}.vzip", env!("CARGO_MANIFEST_DIR"));
    let expected = std::fs::read(path).unwrap();
    let compressed = vzip_compress(input);
    assert!(compressed == expected, "{name}: output differs from SteamKit2's");

    let mut out = vec![0u8; input.len()];
    assert_eq!(vzip_decompress(&compressed, &mut out, true).unwrap(), input.len());
    assert_eq!(out, input);
}

#[test]
fn matches_steamkit2() {
    check("empty", b"");
    check("hello", b"hello world");
    check("fox", "The quick brown fox jumps over the lazy dog. ".repeat(25).as_bytes());
    check("random", &random(20_000, 7));
    check("words", &words(1 << 20, 42));
}

/// 16 MiB, past the match finder's buffer, so it moves its window. Slow in
/// debug builds: `cargo test --release --test vzip -- --ignored`.
#[test]
#[ignore]
fn matches_steamkit2_large() {
    let compressed = vzip_compress(&words(16 << 20, 99));
    assert_eq!(compressed.len(), 733_159);
    assert_eq!(crc32fast::hash(&compressed), 0xd24f_93aa);
}
