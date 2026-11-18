//! Reads NetHook dumps (SteamKit2's NetHookAnalyzer2): a directory of
//! `<seq>_<in|out>_<emsg>_k_EMsg<name>.bin` files, one message each, as
//! written by NetHook2 or [`steamkit::NetHookListener`].

mod decode;

use std::path::{Path, PathBuf};

pub use decode::Analyzer;

use steamkit::EMsg;

/// Which way a message went, seen from the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Received by the client.
    In,
    /// Sent by the client.
    Out,
}

impl Direction {
    /// `"in"` or `"out"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

/// One message of a dump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The sequence number.
    pub sequence: u32,
    /// The direction.
    pub direction: Direction,
    /// The message type from the file name.
    pub emsg: EMsg,
    /// The file.
    pub path: PathBuf,
}

impl Item {
    /// Reads the message.
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }
}

/// Parses `<seq>_<in|out>_<emsg>...` (the analyzer's
/// `(?<num>\d+)_(?<direction>in|out)_(?<emsg>\d+)`, matched anywhere in the
/// name, ignoring case).
pub fn parse_file_name(name: &str) -> Option<(u32, Direction, EMsg)> {
    let bytes = name.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() || (start > 0 && bytes[start - 1].is_ascii_digit()) {
            continue;
        }
        let num_end = start + digits(start);
        let rest = &name[num_end..];
        let (direction, skip) = if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("_in_") {
            (Direction::In, 4)
        } else if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case("_out_") {
            (Direction::Out, 5)
        } else {
            continue;
        };
        let emsg_start = num_end + skip;
        let emsg_end = emsg_start + digits(emsg_start);
        if emsg_end == emsg_start {
            continue;
        }
        let sequence = name[start..num_end].parse().ok()?;
        let emsg = name[emsg_start..emsg_end].parse().ok()?;
        return Some((sequence, direction, EMsg(emsg)));
    }
    None
}

/// A dump directory.
#[derive(Clone, Debug, Default)]
pub struct Dump {
    /// The messages, by sequence number.
    pub items: Vec<Item>,
}

impl Dump {
    /// Loads every message file in `directory`; other files are skipped.
    pub fn load(directory: &Path) -> std::io::Result<Self> {
        let mut items = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bin")) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if let Some((sequence, direction, emsg)) = parse_file_name(name) {
                items.push(Item { sequence, direction, emsg, path });
            }
        }
        items.sort_by_key(|i| i.sequence);
        Ok(Self { items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(
            parse_file_name("007_in_751_k_EMsgClientLogOnResponse.bin"),
            Some((7, Direction::In, EMsg::ClientLogOnResponse))
        );
        assert_eq!(parse_file_name("12_OUT_5514.bin"), Some((12, Direction::Out, EMsg::ClientLogon)));
        assert_eq!(parse_file_name("prefix 3_in_9805"), Some((3, Direction::In, EMsg::ClientHello)));
        // As steamkit-nethook's `capture_file_name` writes them.
        assert_eq!(
            parse_file_name("001_in_751_ClientLogOnResponse.bin"),
            Some((1, Direction::In, EMsg::ClientLogOnResponse))
        );
        assert_eq!(parse_file_name("1000_out_5514_(null).bin"), Some((1000, Direction::Out, EMsg::ClientLogon)));
        assert_eq!(parse_file_name("readme.bin"), None);
        assert_eq!(parse_file_name("1_sideways_2.bin"), None);
    }
}
