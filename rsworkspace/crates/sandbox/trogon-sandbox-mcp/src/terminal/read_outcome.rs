/// What one terminal read returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadOutcome {
    pub(crate) output: String,
    pub(crate) dropped_bytes: u64,
    pub(crate) missed_frames: u64,
    /// The terminal ended and everything it printed has been read.
    pub(crate) exited: bool,
    pub(crate) exit_code: Option<i32>,
}
