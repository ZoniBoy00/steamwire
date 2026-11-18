//! Depot manifests (SteamKit2's `DepotManifest`): the protobuf format served
//! by the CDN, and the older binary v4 format.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR};

use base64::Engine;
use prost::Message;
use sha1::{Digest, Sha1};
use steamwire_protos::steam::{
    content_manifest_payload::{file_mapping, FileMapping},
    ContentManifestMetadata, ContentManifestPayload, ContentManifestSignature,
};

use crate::enums::EDepotFileFlag;
use crate::error::{Error, Result};
use crate::util::crypto::symmetric_decrypt;

const PAYLOAD_MAGIC: u32 = 0x71F6_17D0;
const METADATA_MAGIC: u32 = 0x1F48_12BE;
const SIGNATURE_MAGIC: u32 = 0x1B81_B817;
const END_OF_MANIFEST_MAGIC: u32 = 0x32C4_15AB;
const BINARY_V4_MAGIC: u32 = 0x1634_9781;

/// The separator that isn't the OS one; file names use the OS separator
/// once known, like SteamKit2 does.
const ALT_SEPARATOR: char = if MAIN_SEPARATOR == '\\' { '/' } else { '\\' };

/// A chunk of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkData {
    /// The SHA-1 of the uncompressed chunk, which is also its ID.
    pub chunk_id: Vec<u8>,
    /// Adler-32 (zero seeded) of the uncompressed chunk.
    pub checksum: u32,
    /// Offset in the file.
    pub offset: u64,
    /// Size on the CDN (encrypted and compressed).
    pub compressed_length: u32,
    /// Size in the file.
    pub uncompressed_length: u32,
}

/// A file in a manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileData {
    /// The path, with the OS separator. Base64 and encrypted until
    /// [`DepotManifest::decrypt_filenames`].
    pub file_name: String,
    /// SHA-1 of the lowercased path with `\` separators.
    pub file_name_hash: Vec<u8>,
    /// The chunks.
    pub chunks: Vec<ChunkData>,
    /// Flags (directory, executable, symlink, ...).
    pub flags: EDepotFileFlag,
    /// The file size.
    pub total_size: u64,
    /// SHA-1 of the file.
    pub file_hash: Vec<u8>,
    /// The target, for symlinks.
    pub link_target: Option<String>,
}

impl FileData {
    fn new(name: String, encrypted: bool) -> Self {
        let file_name = if encrypted { name } else { name.replace(ALT_SEPARATOR, MAIN_SEPARATOR_STR) };
        Self { file_name, ..Default::default() }
    }
}

/// A depot manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DepotManifest {
    /// The files.
    pub files: Vec<FileData>,
    /// Whether file names are still encrypted.
    pub filenames_encrypted: bool,
    /// The depot.
    pub depot_id: u32,
    /// The manifest ID.
    pub manifest_gid: u64,
    /// Unix time the manifest was created.
    pub creation_time: u32,
    /// Total size of the files.
    pub total_uncompressed_size: u64,
    /// Total size of the chunks on the CDN.
    pub total_compressed_size: u64,
    /// CRC of the payload with encrypted names.
    pub encrypted_crc: u32,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or_else(|| Error::InvalidData("manifest is truncated".into()))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn cstring(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::InvalidData("unterminated string in manifest".into()))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }

    fn section<M: Message + Default>(&mut self) -> Result<M> {
        let len = self.u32()? as usize;
        Ok(M::decode(self.take(len)?)?)
    }
}

/// Compares like .NET's `StringComparer.OrdinalIgnoreCase`.
fn cmp_ignore_case(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.chars().flat_map(char::to_uppercase).collect::<Vec<_>>();
    fold(a).cmp(&fold(b))
}

impl DepotManifest {
    /// Parses a manifest (the unzipped CDN response, or a saved manifest).
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let mut r = Cursor { data, pos: 0 };
        let mut payload = None;
        let mut metadata = None;
        let mut signature = None;
        loop {
            match r.u32()? {
                END_OF_MANIFEST_MAGIC => break,
                BINARY_V4_MAGIC => {
                    let manifest = Self::parse_binary_v4(&mut r)?;
                    if r.u32()? != BINARY_V4_MAGIC {
                        return Err(Error::InvalidData("missing end marker of v4 manifest".into()));
                    }
                    return Ok(manifest);
                }
                PAYLOAD_MAGIC => payload = Some(r.section::<ContentManifestPayload>()?),
                METADATA_MAGIC => metadata = Some(r.section::<ContentManifestMetadata>()?),
                SIGNATURE_MAGIC => signature = Some(r.section::<ContentManifestSignature>()?),
                magic => return Err(Error::InvalidData(format!("unknown manifest magic {magic:#X}"))),
            }
        }
        let (Some(payload), Some(metadata), Some(_)) = (payload, metadata, signature) else {
            return Err(Error::InvalidData("manifest is missing sections".into()));
        };

        let encrypted = metadata.filenames_encrypted();
        let files = payload
            .mappings
            .into_iter()
            .map(|m| {
                let mut file = FileData::new(m.filename.clone().unwrap_or_default(), encrypted);
                file.file_name_hash = m.sha_filename.clone().unwrap_or_default();
                file.flags = EDepotFileFlag(m.flags() as i32);
                file.total_size = m.size();
                file.file_hash = m.sha_content.clone().unwrap_or_default();
                file.link_target = m.linktarget.clone().filter(|l| !l.is_empty());
                file.chunks = m
                    .chunks
                    .iter()
                    .map(|c| ChunkData {
                        chunk_id: c.sha.clone().unwrap_or_default(),
                        checksum: c.crc(),
                        offset: c.offset(),
                        compressed_length: c.cb_compressed(),
                        uncompressed_length: c.cb_original(),
                    })
                    .collect();
                file
            })
            .collect();
        Ok(Self {
            files,
            filenames_encrypted: encrypted,
            depot_id: metadata.depot_id(),
            manifest_gid: metadata.gid_manifest(),
            creation_time: metadata.creation_time(),
            total_uncompressed_size: metadata.cb_disk_original(),
            total_compressed_size: metadata.cb_disk_compressed(),
            encrypted_crc: metadata.crc_encrypted(),
        })
    }

    /// The binary v4 format (`Steam3Manifest`), after its magic.
    fn parse_binary_v4(r: &mut Cursor) -> Result<Self> {
        let version = r.u32()?;
        if version != 4 {
            return Err(Error::InvalidData(format!("unsupported binary manifest version {version}")));
        }
        let mut manifest = Self {
            depot_id: r.u32()?,
            manifest_gid: r.u64()?,
            creation_time: r.u32()?,
            filenames_encrypted: r.u32()? != 0,
            total_uncompressed_size: r.u64()?,
            total_compressed_size: r.u64()?,
            ..Default::default()
        };
        let _chunk_count = r.u32()?;
        let _file_entry_count = r.u32()?;
        let mapping_size = r.u32()? as usize;
        manifest.encrypted_crc = r.u32()?;
        let _decrypted_crc = r.u32()?;
        let _flags = r.u32()?;

        let end = r.pos.checked_add(mapping_size).filter(|&e| e <= r.data.len());
        let end = end.ok_or_else(|| Error::InvalidData("file mapping exceeds the manifest".into()))?;
        while r.pos < end {
            let mut file = FileData::new(r.cstring()?, manifest.filenames_encrypted);
            file.total_size = r.u64()?;
            file.flags = EDepotFileFlag(r.u32()? as i32);
            file.file_hash = r.take(20)?.to_vec();
            file.file_name_hash = r.take(20)?.to_vec();
            let chunks = r.u32()?;
            for _ in 0..chunks {
                file.chunks.push(ChunkData {
                    chunk_id: r.take(20)?.to_vec(),
                    checksum: r.u32()?,
                    offset: r.u64()?,
                    uncompressed_length: r.u32()?,
                    compressed_length: r.u32()?,
                });
            }
            manifest.files.push(file);
        }
        if r.pos != end {
            return Err(Error::InvalidData("file mapping exceeds its declared size".into()));
        }
        Ok(manifest)
    }

    /// Decrypts file names (and symlink targets) with the depot key, then
    /// sorts the files by name like Steam. Does nothing if they're not
    /// encrypted.
    pub fn decrypt_filenames(&mut self, depot_key: &[u8]) -> Result<()> {
        if !self.filenames_encrypted {
            return Ok(());
        }
        for file in &mut self.files {
            file.file_name = decrypt_name(&file.file_name, depot_key)?;
            if let Some(target) = file.link_target.as_mut().filter(|t| !t.is_empty()) {
                *target = decrypt_name(target, depot_key)?;
            }
        }
        // Doesn't match Steam for non-ASCII names, like SteamKit2.
        self.files.sort_by(|a, b| cmp_ignore_case(&a.file_name, &b.file_name));
        self.filenames_encrypted = false;
        Ok(())
    }

    /// Reads a manifest saved with [`save_to_file`](Self::save_to_file)
    /// (SteamKit2's `LoadFromFile`).
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::deserialize(&std::fs::read(path)?)
    }

    /// Writes [`serialize`](Self::serialize)'s output to a file (SteamKit2's
    /// `SaveToFile`).
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.serialize())
    }

    /// Writes the manifest in the protobuf format, with an empty signature.
    pub fn serialize(&self) -> Vec<u8> {
        let mut payload = ContentManifestPayload::default();
        let mut unique_chunks = HashSet::new();
        for file in &self.files {
            let (filename, sha_filename) = if self.filenames_encrypted {
                (file.file_name.clone(), file.file_name_hash.clone())
            } else {
                let name = file.file_name.replace('/', "\\");
                let hash = Sha1::digest(name.to_lowercase().as_bytes()).to_vec();
                (name, hash)
            };
            payload.mappings.push(FileMapping {
                filename: Some(filename),
                size: Some(file.total_size),
                flags: Some(file.flags.0 as u32),
                sha_filename: Some(sha_filename),
                sha_content: Some(file.file_hash.clone()),
                chunks: file
                    .chunks
                    .iter()
                    .map(|c| {
                        unique_chunks.insert(c.chunk_id.as_slice());
                        file_mapping::ChunkData {
                            sha: Some(c.chunk_id.clone()),
                            crc: Some(c.checksum),
                            offset: Some(c.offset),
                            cb_original: Some(c.uncompressed_length),
                            cb_compressed: Some(c.compressed_length),
                        }
                    })
                    .collect(),
                linktarget: file.link_target.clone().filter(|l| !l.trim().is_empty()),
            });
        }
        let payload = payload.encode_to_vec();

        let mut crc = crc32fast::Hasher::new();
        crc.update(&(payload.len() as i32).to_le_bytes());
        crc.update(&payload);
        let crc = crc.finalize();
        let (crc_encrypted, crc_clear) = if self.filenames_encrypted { (crc, 0) } else { (self.encrypted_crc, crc) };

        let metadata = ContentManifestMetadata {
            depot_id: Some(self.depot_id),
            gid_manifest: Some(self.manifest_gid),
            creation_time: Some(self.creation_time),
            filenames_encrypted: Some(self.filenames_encrypted),
            cb_disk_original: Some(self.total_uncompressed_size),
            cb_disk_compressed: Some(self.total_compressed_size),
            unique_chunks: Some(unique_chunks.len() as u32),
            crc_encrypted: Some(crc_encrypted),
            crc_clear: Some(crc_clear),
        }
        .encode_to_vec();

        let mut out = Vec::with_capacity(payload.len() + metadata.len() + 32);
        for (magic, section) in [(PAYLOAD_MAGIC, &payload[..]), (METADATA_MAGIC, &metadata[..]), (SIGNATURE_MAGIC, &[])]
        {
            out.extend_from_slice(&magic.to_le_bytes());
            out.extend_from_slice(&(section.len() as u32).to_le_bytes());
            out.extend_from_slice(section);
        }
        out.extend_from_slice(&END_OF_MANIFEST_MAGIC.to_le_bytes());
        out
    }
}

fn decrypt_name(name: &str, key: &[u8]) -> Result<String> {
    // Like .NET's base64 decoder, skip whitespace (names contain newlines).
    let name: String = name.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let encrypted = base64::engine::general_purpose::STANDARD
        .decode(name)
        .map_err(|e| Error::InvalidData(format!("file name is not base64: {e}")))?;
    let mut plain = symmetric_decrypt(&encrypted, key)?;
    if plain.last() == Some(&0) {
        plain.pop();
    }
    let name = String::from_utf8_lossy(&plain).into_owned();
    Ok(name.replace(ALT_SEPARATOR, MAIN_SEPARATOR_STR))
}
