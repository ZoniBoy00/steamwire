//! The binary KeyValues reader.

use super::{KeyValue, ParseError, Value};

pub(super) const BIN_NONE: u8 = 0;
pub(super) const BIN_STRING: u8 = 1;
pub(super) const BIN_INT32: u8 = 2;
pub(super) const BIN_FLOAT32: u8 = 3;
pub(super) const BIN_POINTER: u8 = 4;
pub(super) const BIN_WIDESTRING: u8 = 5;
pub(super) const BIN_COLOR: u8 = 6;
pub(super) const BIN_UINT64: u8 = 7;
pub(super) const BIN_END: u8 = 8;
pub(super) const BIN_INT64: u8 = 10;
pub(super) const BIN_ALTERNATE_END: u8 = 11;

pub(super) struct BinaryReader<'a> {
    pub(super) data: &'a [u8],
    pub(super) pos: usize,
}

impl BinaryReader<'_> {
    fn error(&self, message: &'static str) -> ParseError {
        ParseError { offset: self.pos, message }
    }

    fn byte(&mut self) -> Result<u8, ParseError> {
        let b = *self.data.get(self.pos).ok_or(self.error("unexpected end of data"))?;
        self.pos += 1;
        Ok(b)
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let slice = self.data.get(self.pos..self.pos + N).ok_or(self.error("unexpected end of data"))?;
        self.pos += N;
        Ok(slice.try_into().unwrap())
    }

    fn cstring(&mut self) -> Result<String, ParseError> {
        let rest = &self.data[self.pos..];
        let len = rest.iter().position(|&b| b == 0).ok_or(self.error("unterminated string"))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }

    pub(super) fn read_children(&mut self) -> Result<Vec<KeyValue>, ParseError> {
        let mut children = Vec::new();
        loop {
            if self.pos >= self.data.len() {
                // Some blobs end without the final end marker.
                return Ok(children);
            }
            let ty = self.byte()?;
            if ty == BIN_END || ty == BIN_ALTERNATE_END {
                return Ok(children);
            }
            let name = self.cstring()?;
            let value = match ty {
                BIN_NONE => Value::Children(self.read_children()?),
                BIN_STRING => Value::String(self.cstring()?),
                BIN_WIDESTRING => return Err(self.error("wide strings are not supported")),
                BIN_INT32 => Value::Int32(i32::from_le_bytes(self.bytes()?)),
                BIN_POINTER => Value::Pointer(i32::from_le_bytes(self.bytes()?)),
                BIN_COLOR => Value::Color(i32::from_le_bytes(self.bytes()?)),
                BIN_UINT64 => Value::UInt64(u64::from_le_bytes(self.bytes()?)),
                BIN_INT64 => Value::Int64(i64::from_le_bytes(self.bytes()?)),
                BIN_FLOAT32 => Value::Float32(f32::from_le_bytes(self.bytes()?)),
                _ => return Err(self.error("unknown binary KeyValues type")),
            };
            children.push(KeyValue { name, value });
        }
    }
}
