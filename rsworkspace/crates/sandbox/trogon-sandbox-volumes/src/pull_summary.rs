#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullSummary {
    pub objects: u64,
    pub bytes: u64,
}
