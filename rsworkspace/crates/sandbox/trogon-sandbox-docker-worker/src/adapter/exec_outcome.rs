pub(super) enum SignalOutcome {
    Signaled,
    Gone(i32),
    Uncertain(String),
}

pub(super) enum TimeoutCleanup {
    Killed,
    Exited(i32),
}
