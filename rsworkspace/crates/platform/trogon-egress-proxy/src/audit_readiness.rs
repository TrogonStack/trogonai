#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditReadiness {
    Ready,
    Unavailable,
    Degraded,
    Overloaded,
    TimedOut,
}
