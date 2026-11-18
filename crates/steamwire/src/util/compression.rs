//! Steam's container formats: VZip (LZMA, compression and decompression),
//! VZstd and single-file PKZip archives, plus Steam's zero-seeded Adler-32.

use std::io::Read;

use crate::error::{Error, Result};

fn invalid(msg: impl Into<String>) -> Error {
    Error::InvalidData(msg.into())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Adler-32 as Steam computes chunk checksums: seeded with 0, not 1
/// (SteamKit2's `Adler32.Calculate(0, data)`).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    // 5552 is the largest n with 255n(n+1)/2 + (n+1)(MOD-1) < 2^32.
    let (mut a, mut b) = (0u32, 0u32);
    for block in data.chunks(5552) {
        for &byte in block {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

const VZIP_HEADER: u16 = 0x5A56; // "VZ"
const VZIP_FOOTER: u16 = 0x767A; // "zv"
const VZIP_HEADER_LEN: usize = 7;
const VZIP_FOOTER_LEN: usize = 10;
const LZMA_PROPS_LEN: usize = super::lzma::PROPERTIES_SIZE;

/// The parts of a VZip frame: LZMA properties, the raw LZMA stream (no end
/// marker; the size comes from the footer), and the output CRC and size.
struct VZipFrame<'a> {
    props: &'a [u8],
    compressed: &'a [u8],
    crc: u32,
    size: usize,
}

fn vzip_frame(buffer: &[u8], version: u8) -> Result<VZipFrame<'_>> {
    if buffer.len() < VZIP_HEADER_LEN + LZMA_PROPS_LEN + VZIP_FOOTER_LEN {
        return Err(invalid(format!("VZip data is too short ({} bytes)", buffer.len())));
    }
    if u16_at(buffer, 0) != VZIP_HEADER {
        return Err(invalid("expected VZip header"));
    }
    if buffer[2] != version {
        return Err(invalid(format!("expected VZip version '{}'", version as char)));
    }
    let footer = &buffer[buffer.len() - VZIP_FOOTER_LEN..];
    let size = i32::from_le_bytes(footer[4..8].try_into().unwrap());
    if u16_at(footer, 8) != VZIP_FOOTER {
        return Err(invalid("expected VZip footer"));
    }
    Ok(VZipFrame {
        props: &buffer[VZIP_HEADER_LEN..VZIP_HEADER_LEN + LZMA_PROPS_LEN],
        compressed: &buffer[VZIP_HEADER_LEN + LZMA_PROPS_LEN..buffer.len() - VZIP_FOOTER_LEN],
        crc: u32_at(footer, 0),
        size: usize::try_from(size).map_err(|_| invalid("negative VZip size"))?,
    })
}

/// Decompresses VZip (`VZa`, LZMA) data into `destination`, returning the
/// decompressed size.
pub fn vzip_decompress(buffer: &[u8], destination: &mut [u8], verify_checksum: bool) -> Result<usize> {
    let frame = vzip_frame(buffer, b'a')?;
    let size = frame.size;
    if destination.len() < size {
        return Err(invalid("destination is smaller than the decompressed data"));
    }
    super::lzma::decode(frame.props, frame.compressed, &mut destination[..size])?;
    if verify_checksum && crc32fast::hash(&destination[..size]) != frame.crc {
        return Err(invalid("VZip CRC mismatch"));
    }
    Ok(size)
}

/// Compresses `buffer` into VZip (`VZa`) data, byte for byte as SteamKit2's
/// `VZipUtil.Compress` does (the LZMA SDK encoder with a 8 MiB dictionary,
/// BT4 and 128 fast bytes).
pub fn vzip_compress(buffer: &[u8]) -> Vec<u8> {
    use super::lzma_encoder::{encode, EncoderOptions};

    let crc = crc32fast::hash(buffer).to_le_bytes();
    let mut out = Vec::with_capacity(VZIP_HEADER_LEN + LZMA_PROPS_LEN + buffer.len() / 2 + VZIP_FOOTER_LEN);
    out.extend(VZIP_HEADER.to_le_bytes());
    out.push(b'a');
    out.extend(crc);
    out.extend(encode(buffer, &EncoderOptions::VZIP).expect("VZip options are valid"));
    out.extend(crc);
    out.extend((buffer.len() as u32).to_le_bytes());
    out.extend(VZIP_FOOTER.to_le_bytes());
    out
}

/// The largest VZip delta output accepted.
const MAX_DELTA_SIZE: usize = 64 << 20;

/// Decompresses a VZip delta (`VZd`): LZMA data whose matches may refer to
/// `source`, the previous version of the chunk (SteamKit2's
/// `VZipDeltaUtil`).
pub fn vzip_delta_decompress(buffer: &[u8], source: &[u8]) -> Result<Vec<u8>> {
    let frame = vzip_frame(buffer, b'd')?;
    // The size comes from the footer; depot chunks are at most a few MiB.
    if frame.size > MAX_DELTA_SIZE {
        return Err(invalid(format!("VZip delta claims {} bytes", frame.size)));
    }
    let mut window = vec![0u8; source.len() + frame.size];
    window[..source.len()].copy_from_slice(source);
    super::lzma::decode_with_dictionary(frame.props, frame.compressed, &mut window, source.len())?;
    let out = window.split_off(source.len());
    if crc32fast::hash(&out) != frame.crc {
        return Err(invalid("VZip delta CRC mismatch"));
    }
    Ok(out)
}

const VZSTD_HEADER: u32 = 0x615A_5356; // "VSZa"
const VZSTD_HEADER_LEN: usize = 8;
const VZSTD_FOOTER_LEN: usize = 15;

/// Decompresses VZstd (`VSZa`) data into `destination`, returning the
/// decompressed size.
pub fn vzstd_decompress(buffer: &[u8], destination: &mut [u8], verify_checksum: bool) -> Result<usize> {
    if buffer.len() < VZSTD_HEADER_LEN + VZSTD_FOOTER_LEN {
        return Err(invalid(format!("VZstd data is too short ({} bytes)", buffer.len())));
    }
    if u32_at(buffer, 0) != VZSTD_HEADER {
        return Err(invalid("expected VZstd header"));
    }
    let n = buffer.len();
    let crc = u32_at(buffer, n - VZSTD_FOOTER_LEN);
    let size = i32::from_le_bytes(buffer[n - 11..n - 7].try_into().unwrap());
    if &buffer[n - 3..] != b"zsv" {
        return Err(invalid("expected VZstd footer"));
    }
    let size = usize::try_from(size).map_err(|_| invalid("negative VZstd size"))?;
    if destination.len() < size {
        return Err(invalid("destination is smaller than the decompressed data"));
    }
    let input = &buffer[VZSTD_HEADER_LEN..n - VZSTD_FOOTER_LEN];
    let written = ruzstd::decoding::FrameDecoder::new()
        .decode_all(input, &mut destination[..size])
        .map_err(|e| invalid(format!("zstd: {e}")))?;
    if written != size {
        return Err(invalid(format!("zstd: expected {size} bytes, got {written}")));
    }
    if verify_checksum && crc32fast::hash(&destination[..size]) != crc {
        return Err(invalid("VZstd CRC mismatch"));
    }
    Ok(size)
}

/// Reads the single file in a PKZip archive (stored or deflated), taking
/// sizes from the central directory.
pub fn zip_decompress(buffer: &[u8], verify_checksum: bool) -> Result<Vec<u8>> {
    const EOCD_SIG: u32 = 0x0605_4B50;
    const CDIR_SIG: u32 = 0x0201_4B50;
    const LOCAL_SIG: u32 = 0x0403_4B50;

    if buffer.len() < 22 {
        return Err(invalid("zip: too short"));
    }
    let eocd = (0..=buffer.len() - 22)
        .rev()
        .find(|&i| u32_at(buffer, i) == EOCD_SIG)
        .ok_or_else(|| invalid("zip: no end of central directory"))?;
    let entries = u16_at(buffer, eocd + 10);
    if entries != 1 {
        tracing::debug!(entries, "expected one file in the zip");
    }
    let cdir = u32_at(buffer, eocd + 16) as usize;
    if cdir + 46 > buffer.len() || u32_at(buffer, cdir) != CDIR_SIG {
        return Err(invalid("zip: bad central directory"));
    }
    let method = u16_at(buffer, cdir + 10);
    let crc = u32_at(buffer, cdir + 16);
    let compressed_size = u32_at(buffer, cdir + 20) as usize;
    let size = u32_at(buffer, cdir + 24) as usize;
    let local = u32_at(buffer, cdir + 42) as usize;
    if local + 30 > buffer.len() || u32_at(buffer, local) != LOCAL_SIG {
        return Err(invalid("zip: bad local header"));
    }
    let start = local + 30 + u16_at(buffer, local + 26) as usize + u16_at(buffer, local + 28) as usize;
    let data = buffer.get(start..start + compressed_size).ok_or_else(|| invalid("zip: truncated data"))?;

    let out = match method {
        0 => data.to_vec(),
        8 => {
            let mut out = Vec::with_capacity(size.min(16 << 20));
            flate2::read::DeflateDecoder::new(data)
                .read_to_end(&mut out)
                .map_err(|e| invalid(format!("deflate: {e}")))?;
            out
        }
        m => return Err(invalid(format!("zip: unsupported method {m}"))),
    };
    if out.len() != size {
        return Err(invalid("zip: size mismatch"));
    }
    if verify_checksum && crc32fast::hash(&out) != crc {
        return Err(invalid("zip: CRC mismatch"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler_is_zero_seeded() {
        assert_eq!(adler32(&[]), 0);
        // Standard Adler-32 of "a" is 0x00620062; seeded with 0 it's 0x00610061.
        assert_eq!(adler32(b"a"), 0x0061_0061);
        let big = vec![0xFFu8; 100_000];
        let (mut a, mut b) = (0u64, 0u64);
        for &x in &big {
            a = (a + x as u64) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&big), ((b << 16) | a) as u32);
    }

    /// A delta whose target is LZMA-encoded on its own, so the source only
    /// provides context, not matches.
    #[test]
    fn vzip_delta() {
        use crate::util::lzma_encoder::{encode, EncoderOptions};

        // lc=3 contexts use the previous byte >> 5 and pb=2 the position & 3;
        // the encoder starts at position 0 after a zero byte, so the source
        // ends in a byte below 32 and has a length divisible by 4.
        let source: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 31) as u8).collect();
        let target: Vec<u8> = (0..700u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let encoded = encode(&target, &EncoderOptions::VZIP).unwrap();
        let crc = crc32fast::hash(&target);
        let mut delta = vec![b'V', b'Z', b'd'];
        delta.extend(crc.to_le_bytes());
        delta.extend(encoded);
        delta.extend(crc.to_le_bytes());
        delta.extend((target.len() as u32).to_le_bytes());
        delta.extend(b"zv");

        assert_eq!(vzip_delta_decompress(&delta, &source).unwrap(), target);
        let mut corrupt = delta.clone();
        let footer = corrupt.len() - 10;
        corrupt[footer] ^= 1;
        assert!(vzip_delta_decompress(&corrupt, &source).is_err());
        // A plain VZip is not a delta.
        delta[2] = b'a';
        assert!(vzip_delta_decompress(&delta, &source).is_err());
    }

    #[test]
    fn zip_with_data_descriptor() {
        use std::io::Write;
        let payload = b"hello hello hello hello";
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(payload).unwrap();
        let deflated = enc.finish().unwrap();
        let crc = crc32fast::hash(payload);

        let mut zip = Vec::new();
        zip.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        // version, flags (bit 3: sizes in a data descriptor), method, time, date
        zip.extend_from_slice(&[20, 0, 8, 0, 8, 0, 0, 0, 0, 0]);
        zip.extend_from_slice(&[0; 12]);
        zip.extend_from_slice(&1u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.push(b'z');
        zip.extend_from_slice(&deflated);
        let cdir = zip.len() as u32;
        zip.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        zip.extend_from_slice(&[20, 0, 20, 0, 8, 0, 8, 0, 0, 0, 0, 0]);
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        zip.extend_from_slice(&1u16.to_le_bytes());
        zip.extend_from_slice(&[0; 12]);
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.push(b'z');
        let cdir_len = zip.len() as u32 - cdir;
        zip.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        zip.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
        zip.extend_from_slice(&cdir_len.to_le_bytes());
        zip.extend_from_slice(&cdir.to_le_bytes());
        zip.extend_from_slice(&[0, 0]);

        assert_eq!(zip_decompress(&zip, true).unwrap(), payload);
    }
}
