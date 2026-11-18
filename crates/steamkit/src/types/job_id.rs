/// A job ID, used to match a request with its response. Job IDs are
/// [`GlobalId`](super::GlobalId)s; [`JobId::global`] gives that view.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct JobId(pub u64);

impl JobId {
    /// "No job" (`ulong.MaxValue`).
    pub const NONE: JobId = JobId(u64::MAX);

    /// Whether this is [`JobId::NONE`].
    pub const fn is_none(self) -> bool {
        self.0 == u64::MAX
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::NONE
    }
}

impl From<u64> for JobId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl JobId {
    /// The job ID as a [`GlobalId`](super::GlobalId).
    pub const fn global(self) -> super::GlobalId {
        super::GlobalId(self.0)
    }
}
