/// State of a mutating session operation covered by a Session Lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOperationState {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    RequiresReconciliation,
}

/// Session recovery lifecycle when reconstructing state from the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryState {
    Pending,
    Replaying,
    Completed,
    Failed,
    StaleSnapshot,
}

/// Session lease lifecycle for a mutating operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    Idle,
    Acquired,
    Renewing,
    Expired,
    Contended,
}

/// Product-facing response when a session is busy with another mutating operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBusyPolicy {
    SessionBusy,
}
