//! Finds protobuf file descriptors compiled into binaries and turns them
//! back into `.proto` files (SteamKit2's ProtobufDumper).
//!
//! Binaries built with the full protobuf runtime embed every
//! `FileDescriptorProto`, serialized. Each starts with field 1, the file
//! name: `0A <len> <name>.proto`, which is what the scanner looks for.

mod render;

use std::collections::HashSet;

use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::FileDescriptorProto;

use crate::wire::varint;

pub use render::render_file;

/// A name the scanner accepts (`^[a-zA-Z_0-9\\/.-]+\.proto$`).
fn is_proto_name(name: &[u8]) -> bool {
    name.len() > ".proto".len()
        && name[name.len() - 6..].eq_ignore_ascii_case(b".proto")
        && name.iter().all(|&c| c.is_ascii_alphanumeric() || b"_\\/.-".contains(&c))
}

/// The length of the message at the start of `data`: fields are read until
/// one can't be, or field 1 (the file name) appears a second time, which
/// is the start of the next descriptor.
fn message_length(data: &[u8]) -> usize {
    let mut pos = 0;
    let mut seen_name = false;
    loop {
        let start = pos;
        let Some((key, n)) = varint(&data[pos..]) else { return start };
        pos += n;
        let field = key >> 3;
        if field == 0 {
            return start;
        }
        if field == 1 {
            if seen_name {
                return start;
            }
            seen_name = true;
        }
        let skip = match key & 7 {
            0 => varint(&data[pos..]).map(|(_, n)| n),
            1 => Some(8),
            2 => varint(&data[pos..]).and_then(|(len, n)| n.checked_add(usize::try_from(len).ok()?)),
            5 => Some(4),
            _ => None,
        };
        match skip {
            Some(skip) if pos + skip <= data.len() => pos += skip,
            _ => return start,
        }
    }
}

/// A file descriptor found in a binary.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The decoded descriptor. Custom options are lost in it, as `prost`
    /// drops unknown fields; [`build_pool`] uses [`Found::raw`].
    pub proto: FileDescriptorProto,
    /// The serialized descriptor.
    pub raw: Vec<u8>,
}

/// Every file descriptor found in `data`, in order, without duplicates.
pub fn scan(data: &[u8]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut names = HashSet::new();
    let mut i = 0;
    while i + 1 < data.len() {
        let Some(at) = data[i..].iter().position(|&b| b == 0x0A) else { break };
        i += at;
        if i + 1 >= data.len() {
            break;
        }
        // The name length is a varint; names of 128+ characters take two bytes.
        let (mut len, mut marker) = ((data[i + 1] & 0x7F) as usize, 2);
        if data[i + 1] & 0x80 != 0 {
            if i + 2 >= data.len() || data[i + 2] & 0x80 != 0 {
                i += 1;
                continue;
            }
            len |= (data[i + 2] as usize) << 7;
            marker = 3;
        }
        let Some(name) = data.get(i + marker..i + marker + len) else {
            i += 1;
            continue;
        };
        if !is_proto_name(name) {
            i += 1;
            continue;
        }
        let length = message_length(&data[i..]);
        let raw = &data[i..i + length];
        match FileDescriptorProto::decode(raw) {
            Ok(proto) if length > 0 => {
                if names.insert(proto.name().to_owned()) {
                    found.push(Found { proto, raw: raw.to_vec() });
                }
                i += length;
            }
            _ => i += 1,
        }
    }
    found
}

/// The result of [`build_pool`].
pub struct Collected {
    /// The pool, with the well-known Google files and every file that could
    /// be added.
    pub pool: DescriptorPool,
    /// The files that were added, in dependency order.
    pub files: Vec<String>,
    /// Files that couldn't be added, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Adds the files to a pool, dependencies first. Files missing a
/// dependency (or invalid) are reported in [`Collected::failed`].
pub fn build_pool(files: Vec<Found>) -> Collected {
    let mut pool = DescriptorPool::global();
    let mut pending: Vec<Found> =
        files.into_iter().filter(|f| pool.get_file_by_name(f.proto.name()).is_none()).collect();
    let mut added = Vec::new();
    loop {
        let before = pending.len();
        let mut rest = Vec::new();
        for file in pending {
            let ready = file.proto.dependency.iter().all(|d| pool.get_file_by_name(d).is_some());
            if !ready {
                rest.push(file);
                continue;
            }
            let name = file.proto.name().to_owned();
            match pool.decode_file_descriptor_proto(&file.raw[..]) {
                Ok(()) => added.push(name),
                Err(_) => rest.push(file),
            }
        }
        pending = rest;
        if pending.len() == before || pending.is_empty() {
            break;
        }
    }
    let failed = pending
        .into_iter()
        .map(|f| {
            let missing: Vec<_> =
                f.proto.dependency.iter().filter(|d| pool.get_file_by_name(d).is_none()).cloned().collect();
            let reason = if missing.is_empty() {
                "invalid descriptor".to_owned()
            } else {
                format!("missing {}", missing.join(", "))
            };
            (f.proto.name().to_owned(), reason)
        })
        .collect();
    Collected { pool, files: added, failed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_proto_name(b"steammessages_base.proto"));
        assert!(is_proto_name(b"google/protobuf/descriptor.PROTO"));
        assert!(!is_proto_name(b".proto"));
        assert!(!is_proto_name(b"bad name.proto"));
    }

    #[test]
    fn scans_padded_descriptors() {
        let a = FileDescriptorProto { name: Some("a.proto".into()), package: Some("x".into()), ..Default::default() };
        let b = FileDescriptorProto {
            name: Some("b.proto".into()),
            dependency: vec!["a.proto".into()],
            ..Default::default()
        };
        let mut data = vec![0x0A, 0x03, 0xFF, 0x00];
        data.extend(a.encode_to_vec());
        data.extend([0x0A, 0x0A, 0x00]);
        data.extend(b.encode_to_vec());
        data.extend(a.encode_to_vec());
        let found: Vec<_> = scan(&data).into_iter().map(|f| f.proto).collect();
        assert_eq!(found, [a, b]);
    }
}
