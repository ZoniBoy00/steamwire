//! Protobuf wire format helpers shared by the analyzer and the dumper.

/// Reads a base-128 varint: the value and its length in bytes.
pub(crate) fn varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &b) in data.iter().enumerate().take(10) {
        value |= u64::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::varint;

    #[test]
    fn varints() {
        assert_eq!(varint(&[0x08]), Some((8, 1)));
        assert_eq!(varint(&[0xAC, 0x02, 0xFF]), Some((300, 2)));
        assert_eq!(varint(&[0x80]), None);
    }
}
