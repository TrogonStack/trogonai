/// Who paused a sandbox, so a thaw never resumes a pause somebody else owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PauseOwnership {
    Unowned,
    Manual,
    AuthorityPending,
    Authority,
}

impl PauseOwnership {
    pub(crate) fn is_authority(self) -> bool {
        matches!(self, Self::AuthorityPending | Self::Authority)
    }
}
