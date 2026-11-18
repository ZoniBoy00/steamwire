use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds from the Unix epoch to 2005-01-01, the epoch of [`GlobalId`].
const GID_EPOCH: u64 = 1_104_537_600;

/// A Steam global ID: sequential count (20 bits), start time in seconds since
/// 2005-01-01 (30 bits), process ID (4 bits) and box ID (10 bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalId(pub u64);

impl GlobalId {
    /// The invalid ID (`u64::MAX`).
    pub const INVALID: GlobalId = GlobalId(u64::MAX);

    fn bits(self, shift: u32, mask: u64) -> u64 {
        (self.0 >> shift) & mask
    }

    fn set_bits(&mut self, shift: u32, mask: u64, value: u64) {
        self.0 = (self.0 & !(mask << shift)) | ((value & mask) << shift);
    }

    /// The sequential count.
    pub fn sequential_count(self) -> u32 {
        self.bits(0, 0xF_FFFF) as u32
    }

    /// Sets the sequential count.
    pub fn set_sequential_count(&mut self, v: u32) {
        self.set_bits(0, 0xF_FFFF, v as u64);
    }

    /// The start time.
    pub fn start_time(self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(GID_EPOCH + self.bits(20, 0x3FFF_FFFF))
    }

    /// Sets the start time.
    pub fn set_start_time(&mut self, t: SystemTime) {
        let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.set_bits(20, 0x3FFF_FFFF, secs.saturating_sub(GID_EPOCH));
    }

    /// The process ID.
    pub fn process_id(self) -> u32 {
        self.bits(50, 0xF) as u32
    }

    /// Sets the process ID.
    pub fn set_process_id(&mut self, v: u32) {
        self.set_bits(50, 0xF, v as u64);
    }

    /// The box ID.
    pub fn box_id(self) -> u32 {
        self.bits(54, 0x3FF) as u32
    }

    /// Sets the box ID.
    pub fn set_box_id(&mut self, v: u32) {
        self.set_bits(54, 0x3FF, v as u64);
    }
}

impl Default for GlobalId {
    fn default() -> Self {
        Self::INVALID
    }
}

impl fmt::Debug for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalId")
            .field("value", &self.0)
            .field("sequential_count", &self.sequential_count())
            .field("process_id", &self.process_id())
            .field("box_id", &self.box_id())
            .finish()
    }
}

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for GlobalId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// A handle to user generated content (cloud files, screenshots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct UgcHandle(pub GlobalId);

impl From<u64> for UgcHandle {
    fn from(v: u64) -> Self {
        Self(GlobalId(v))
    }
}

/// A Steam Workshop published file ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublishedFileId(pub u64);

impl Default for PublishedFileId {
    fn default() -> Self {
        Self(u64::MAX)
    }
}

impl From<u64> for PublishedFileId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for PublishedFileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields() {
        let mut gid = GlobalId(0);
        gid.set_sequential_count(12345);
        gid.set_process_id(7);
        gid.set_box_id(1000);
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        gid.set_start_time(t);
        assert_eq!(gid.sequential_count(), 12345);
        assert_eq!(gid.process_id(), 7);
        assert_eq!(gid.box_id(), 1000);
        assert_eq!(gid.start_time(), t);
        assert_eq!(GlobalId::default(), GlobalId::INVALID);
    }
}
