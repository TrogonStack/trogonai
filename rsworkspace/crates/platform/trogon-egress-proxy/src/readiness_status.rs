use crate::audit_readiness::AuditReadiness;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessStatus {
    Ready,
    Starting,
    Stopped,
    UnsupportedProfile,
    IdentityUnavailable,
    PolicyUnavailable,
    Audit(AuditReadiness),
}
