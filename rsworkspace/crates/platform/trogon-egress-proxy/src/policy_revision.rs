use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyRevision {
    pub incarnation: NonZeroU64,
    pub generation: NonZeroU64,
}
