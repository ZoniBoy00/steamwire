//! Depot chunk processing (SteamKit2's `DepotChunk`).

use super::manifest::ChunkData;
use crate::error::{Error, Result};
use crate::util::compression::{adler32, vzip_decompress, vzstd_decompress, zip_decompress};
use crate::util::crypto::symmetric_decrypt;

/// Decrypts and decompresses a chunk downloaded from the CDN into
/// `destination`, which must hold [`ChunkData::uncompressed_length`] bytes.
/// Returns the number of bytes written. The length and Adler-32 checksum
/// are checked against `info`.
pub fn process(info: &ChunkData, data: &[u8], destination: &mut [u8], depot_key: &[u8]) -> Result<usize> {
    let expected = info.uncompressed_length as usize;
    if destination.len() < expected {
        return Err(Error::Other(format!(
            "destination ({} bytes) is smaller than the chunk ({expected} bytes)",
            destination.len()
        )));
    }
    let decrypted = symmetric_decrypt(data, depot_key)?;
    if decrypted.len() < 16 {
        return Err(Error::InvalidData(format!("decrypted chunk is too short ({} bytes)", decrypted.len())));
    }

    let written = if decrypted.starts_with(b"VSZa") {
        vzstd_decompress(&decrypted, destination, false)?
    } else if decrypted.starts_with(b"VZa") {
        vzip_decompress(&decrypted, destination, false)?
    } else if decrypted.starts_with(b"PK\x03\x04") {
        let out = zip_decompress(&decrypted, false)?;
        destination
            .get_mut(..out.len())
            .ok_or_else(|| Error::InvalidData("zipped chunk is larger than expected".into()))?
            .copy_from_slice(&out);
        out.len()
    } else {
        return Err(Error::InvalidData(format!(
            "unknown chunk compression (starts with {})",
            hex::encode_upper(&decrypted[..4])
        )));
    };

    if written != expected {
        return Err(Error::InvalidData(format!("chunk decompressed to {written} bytes, expected {expected}")));
    }
    if adler32(&destination[..written]) != info.checksum {
        return Err(Error::InvalidData("chunk checksum mismatch: corrupt download or wrong depot key".into()));
    }
    Ok(written)
}

/// Like [`process`], returning a new buffer.
pub fn process_to_vec(info: &ChunkData, data: &[u8], depot_key: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![0u8; info.uncompressed_length as usize];
    let n = process(info, data, &mut out, depot_key)?;
    out.truncate(n);
    Ok(out)
}
