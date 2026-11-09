#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantAdmissionDecision {
    Permit,
    Deny,
}
