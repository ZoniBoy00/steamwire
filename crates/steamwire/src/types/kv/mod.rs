//! Valve's KeyValues format (VDF): the text form and the binary form used in
//! PICS app/package info and some Web API responses.
//!
//! ```
//! use steamwire::kv::KeyValue;
//!
//! let kv = KeyValue::parse_text(r#""app" { "name" "Spacewar" "appid" "480" }"#).unwrap();
//! assert_eq!(kv["name"].as_str(), Some("Spacewar"));
//! assert_eq!(kv["appid"].as_u32(), Some(480));
//! assert!(kv["missing"].is_invalid());
//! ```

mod binary;
mod text;

use std::fmt::{self, Write as _};

use binary::*;
use std::ops::Index;
use text::TextParser;

/// The value of a [`KeyValue`].
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A node with children.
    Children(Vec<KeyValue>),
    /// A string.
    String(String),
    /// A 32-bit integer (binary format only).
    Int32(i32),
    /// A 64-bit unsigned integer (binary format only).
    UInt64(u64),
    /// A 64-bit integer (binary format only).
    Int64(i64),
    /// A 32-bit float (binary format only).
    Float32(f32),
    /// A pointer (binary format only).
    Pointer(i32),
    /// A color (binary format only).
    Color(i32),
}

/// A KeyValues node: a name and either a value or a list of children.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyValue {
    /// The key.
    pub name: String,
    /// The value or children.
    pub value: Value,
}

static INVALID: KeyValue = KeyValue { name: String::new(), value: Value::Children(Vec::new()) };

/// A KeyValues parse error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("KeyValues parse error at byte {offset}: {message}")]
pub struct ParseError {
    /// Byte offset of the error.
    pub offset: usize,
    /// What went wrong.
    pub message: &'static str,
}

impl KeyValue {
    /// A node with children.
    pub fn new_object(name: impl Into<String>, children: Vec<KeyValue>) -> Self {
        Self { name: name.into(), value: Value::Children(children) }
    }

    /// A node with a string value.
    pub fn new_string(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self { name: name.into(), value: Value::String(value.into()) }
    }

    /// The shared "missing" node returned by indexing with an unknown key.
    pub fn invalid() -> &'static KeyValue {
        &INVALID
    }

    /// Whether this is the "missing" node returned by indexing.
    pub fn is_invalid(&self) -> bool {
        std::ptr::eq(self, &INVALID)
    }

    /// The children of this node (empty for a value node).
    pub fn children(&self) -> &[KeyValue] {
        match &self.value {
            Value::Children(c) => c,
            _ => &[],
        }
    }

    /// The children of this node, turning a value node into an empty object.
    pub fn children_mut(&mut self) -> &mut Vec<KeyValue> {
        if !matches!(self.value, Value::Children(_)) {
            self.value = Value::Children(Vec::new());
        }
        match &mut self.value {
            Value::Children(c) => c,
            _ => unreachable!(),
        }
    }

    /// The first child with the given name (case-insensitive, like SteamKit).
    pub fn get(&self, name: &str) -> Option<&KeyValue> {
        self.children().iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The value as a string. Numbers are not converted; use
    /// [`KeyValue::to_value_string`] for that.
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The value rendered as a string, if this is a value node.
    pub fn to_value_string(&self) -> Option<String> {
        Some(match &self.value {
            Value::Children(_) => return None,
            Value::String(s) => s.clone(),
            Value::Int32(v) | Value::Pointer(v) | Value::Color(v) => v.to_string(),
            Value::UInt64(v) => v.to_string(),
            Value::Int64(v) => v.to_string(),
            Value::Float32(v) => v.to_string(),
        })
    }

    /// The value as an `i64`, parsing strings.
    pub fn as_i64(&self) -> Option<i64> {
        match &self.value {
            Value::String(s) => s.trim().parse().ok(),
            Value::Int32(v) | Value::Pointer(v) | Value::Color(v) => Some(*v as i64),
            Value::UInt64(v) => i64::try_from(*v).ok(),
            Value::Int64(v) => Some(*v),
            Value::Float32(_) | Value::Children(_) => None,
        }
    }

    /// The value as a `u64`, parsing strings.
    pub fn as_u64(&self) -> Option<u64> {
        match &self.value {
            Value::UInt64(v) => Some(*v),
            Value::String(s) => s.trim().parse().ok(),
            _ => self.as_i64().and_then(|v| u64::try_from(v).ok()),
        }
    }

    /// The value as a `u32`, parsing strings.
    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().and_then(|v| u32::try_from(v).ok())
    }

    /// The value as an `i32`, parsing strings.
    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }

    /// The value as an `f32`, parsing strings.
    pub fn as_f32(&self) -> Option<f32> {
        match &self.value {
            Value::Float32(v) => Some(*v),
            Value::String(s) => s.trim().parse().ok(),
            _ => self.as_i64().map(|v| v as f32),
        }
    }

    /// The value as a `u8`, parsing strings.
    pub fn as_u8(&self) -> Option<u8> {
        self.as_u64().and_then(|v| u8::try_from(v).ok())
    }

    /// The value as a `u16`, parsing strings.
    pub fn as_u16(&self) -> Option<u16> {
        self.as_u64().and_then(|v| u16::try_from(v).ok())
    }

    /// The value as an enum: a member name (`"Windows10"`) or a number, like
    /// SteamKit2's `AsEnum`.
    pub fn as_enum<T: crate::enums::SteamEnum>(&self) -> Option<T> {
        if let Some(s) = self.as_str() {
            if let Some(value) = T::from_name(s.trim()) {
                return Some(value);
            }
        }
        T::from_i64(self.as_i64()?)
    }

    /// The value as a bool: a non-zero number is `true`.
    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|v| v != 0)
    }

    // ----------------------------------------------------------------- text

    /// Parses the text format. The input must hold one root node.
    pub fn parse_text(input: &str) -> Result<KeyValue, ParseError> {
        let mut p = TextParser { src: input.as_bytes(), pos: 0 };
        // A UTF-8 BOM.
        if p.src.starts_with(&[0xEF, 0xBB, 0xBF]) {
            p.pos = 3;
        }
        let kv = p.read_pair()?.ok_or(p.error("expected a key"))?;
        Ok(kv)
    }

    /// Renders the node in the text format.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out, 0);
        out
    }

    fn write_text(&self, out: &mut String, depth: usize) {
        let indent = "\t".repeat(depth);
        match &self.value {
            Value::Children(children) => {
                let _ = writeln!(out, "{indent}\"{}\"", escape(&self.name));
                let _ = writeln!(out, "{indent}{{");
                for child in children {
                    child.write_text(out, depth + 1);
                }
                let _ = writeln!(out, "{indent}}}");
            }
            _ => {
                let value = self.to_value_string().unwrap_or_default();
                let _ = writeln!(out, "{indent}\"{}\"\t\t\"{}\"", escape(&self.name), escape(&value));
            }
        }
    }

    // ---------------------------------------------------------------- files

    /// Reads a text KeyValues file (SteamKit2's `LoadAsText`).
    pub fn load_text(path: impl AsRef<std::path::Path>) -> crate::Result<KeyValue> {
        let text = std::fs::read_to_string(path)?;
        Self::parse_text(&text).map_err(|e| crate::Error::InvalidData(e.to_string()))
    }

    /// Reads a binary KeyValues file (SteamKit2's `TryLoadAsBinary`).
    pub fn load_binary(path: impl AsRef<std::path::Path>) -> crate::Result<KeyValue> {
        let data = std::fs::read(path)?;
        Self::parse_binary(&data).map_err(|e| crate::Error::InvalidData(e.to_string()))
    }

    /// Writes the node to a file, as text or binary (SteamKit2's
    /// `SaveToFile`).
    pub fn save_to_file(&self, path: impl AsRef<std::path::Path>, binary: bool) -> std::io::Result<()> {
        if binary {
            std::fs::write(path, self.to_binary())
        } else {
            std::fs::write(path, self.to_text())
        }
    }

    // --------------------------------------------------------------- binary

    /// Parses the binary format: one root node, like SteamKit's
    /// `KeyValue.TryReadAsBinary`.
    pub fn parse_binary(data: &[u8]) -> Result<KeyValue, ParseError> {
        Self::parse_binary_all(data)?
            .into_iter()
            .next()
            .ok_or(ParseError { offset: 0, message: "empty binary KeyValues" })
    }

    /// Reads binary children up to their end marker, like SteamKit's
    /// `TryReadAsBinary` on a stream: returns an unnamed node holding them and
    /// the number of bytes used, so several nodes can be read back to back.
    pub fn parse_binary_prefix(data: &[u8]) -> Result<(KeyValue, usize), ParseError> {
        let mut r = BinaryReader { data, pos: 0 };
        let children = r.read_children()?;
        Ok((KeyValue::new_object("", children), r.pos))
    }

    /// Parses every top-level node in the binary format.
    pub fn parse_binary_all(data: &[u8]) -> Result<Vec<KeyValue>, ParseError> {
        BinaryReader { data, pos: 0 }.read_children()
    }

    /// Renders the node in the binary format.
    pub fn to_binary(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_binary(&mut out);
        out.push(BIN_END);
        out
    }

    fn write_binary(&self, out: &mut Vec<u8>) {
        let ty = match &self.value {
            Value::Children(_) => BIN_NONE,
            Value::String(_) => BIN_STRING,
            Value::Int32(_) => BIN_INT32,
            Value::Pointer(_) => BIN_POINTER,
            Value::Color(_) => BIN_COLOR,
            Value::UInt64(_) => BIN_UINT64,
            Value::Int64(_) => BIN_INT64,
            Value::Float32(_) => BIN_FLOAT32,
        };
        out.push(ty);
        out.extend_from_slice(self.name.as_bytes());
        out.push(0);
        match &self.value {
            Value::Children(children) => {
                for child in children {
                    child.write_binary(out);
                }
                out.push(BIN_END);
            }
            Value::String(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Int32(v) | Value::Pointer(v) | Value::Color(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::UInt64(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::Int64(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::Float32(v) => out.extend_from_slice(&v.to_le_bytes()),
        }
    }
}

impl Index<&str> for KeyValue {
    type Output = KeyValue;

    /// The first child with the given name, or the invalid node.
    fn index(&self, name: &str) -> &KeyValue {
        self.get(name).unwrap_or(&INVALID)
    }
}

impl fmt::Display for KeyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
// A comment
"appinfo"
{
    "appid"     "480"
    "common"
    {
        "name"  "Spacewar"   [$WIN32]
        "type"  "Game"
        "escaped" "a\"b\\c"
    }
    unquoted value
}
"#;

    #[test]
    fn parse_text() {
        let kv = KeyValue::parse_text(SAMPLE).unwrap();
        assert_eq!(kv.name, "appinfo");
        assert_eq!(kv["appid"].as_u32(), Some(480));
        assert_eq!(kv["COMMON"]["name"].as_str(), Some("Spacewar"));
        assert_eq!(kv["common"]["escaped"].as_str(), Some("a\"b\\c"));
        assert_eq!(kv["unquoted"].as_str(), Some("value"));
        assert!(kv["nope"]["deeper"].is_invalid());
    }

    #[test]
    fn text_round_trip() {
        let kv = KeyValue::parse_text(SAMPLE).unwrap();
        let again = KeyValue::parse_text(&kv.to_text()).unwrap();
        assert_eq!(kv, again);
    }

    #[test]
    fn binary_round_trip() {
        let kv = KeyValue::new_object(
            "root",
            vec![
                KeyValue::new_string("name", "Spacewar"),
                KeyValue { name: "int".into(), value: Value::Int32(-5) },
                KeyValue { name: "big".into(), value: Value::UInt64(u64::MAX) },
                KeyValue { name: "f".into(), value: Value::Float32(1.5) },
                KeyValue::new_object("child", vec![KeyValue::new_string("a", "b")]),
            ],
        );
        let bin = kv.to_binary();
        let parsed = KeyValue::parse_binary(&bin).unwrap();
        assert_eq!(parsed, kv);
        assert_eq!(parsed["int"].as_i32(), Some(-5));
    }

    #[test]
    fn enums_by_name_or_number() {
        use crate::EResult;
        let kv = KeyValue::parse_text(r#""r" { "a" "OK" "b" "2" "c" "NoSuchName" "d" "-1" }"#).unwrap();
        assert_eq!(kv["a"].as_enum::<EResult>(), Some(EResult::OK));
        assert_eq!(kv["b"].as_enum::<EResult>(), Some(EResult::Fail));
        assert_eq!(kv["c"].as_enum::<EResult>(), None);
        // Unknown numbers are kept, as C#'s Enum.TryParse does.
        assert_eq!(kv["d"].as_enum::<EResult>(), Some(EResult(-1)));
        assert_eq!(kv["b"].as_u8(), Some(2));
        assert_eq!(kv["d"].as_u16(), None);
    }

    #[test]
    fn files_round_trip() {
        let kv = KeyValue::parse_text(SAMPLE).unwrap();
        let dir = std::env::temp_dir().join(format!("steamwire-kv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (text, binary) = (dir.join("kv.txt"), dir.join("kv.bin"));
        kv.save_to_file(&text, false).unwrap();
        kv.save_to_file(&binary, true).unwrap();
        assert_eq!(KeyValue::load_text(&text).unwrap(), kv);
        assert_eq!(KeyValue::load_binary(&binary).unwrap(), kv);
        assert!(KeyValue::load_binary(&text).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn errors() {
        assert!(KeyValue::parse_text("\"a\" {").is_err());
        assert!(KeyValue::parse_text("\"a\" \"unterminated").is_err());
        assert!(KeyValue::parse_binary(&[1, b'a']).is_err());
    }
}
