/// A commit was refused because its revision is not newer than the committed one.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("revision {offered} is not newer than the committed revision {current}")]
pub struct StaleCommit {
    pub current: u64,
    pub offered: u64,
}
