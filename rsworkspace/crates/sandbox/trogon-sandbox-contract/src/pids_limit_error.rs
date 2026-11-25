/// A number is not a process ceiling a lease may ask for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("pids_limit must be between 1 and {max}, not {got}")]
pub struct PidsLimitError {
    pub max: u32,
    pub got: u32,
}
