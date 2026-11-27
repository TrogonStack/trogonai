use trogon_sandbox_contract::StopSignal;

/// How stopping a detached command ended.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum StopOutcome {
    Stopped(StopSignal),
    AlreadyCompleted,
    NotFound,
    StillRunning,
}
