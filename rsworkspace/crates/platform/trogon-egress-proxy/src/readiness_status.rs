#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessStatus {
    Ready,
    Starting,
    Stopped,
    UnsupportedProfile,
    IdentityUnavailable,
    PolicyUnavailable,
    AuditUnavailable,
    AuditDegraded,
    AuditOverloaded,
    AuditTimedOut,
}
