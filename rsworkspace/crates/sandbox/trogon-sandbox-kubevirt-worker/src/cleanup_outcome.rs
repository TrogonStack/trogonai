/// What a confirmed delete found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    Deleted,
    Absent,
}
