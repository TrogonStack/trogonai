use super::{OpenBaoPolicyName, OpenBaoTokenRole};

/// How each release read gets its own short-lived OpenBao token.
///
/// The child is minted through `role` carrying only `policy`, the one fixed
/// read-only policy over the platform's credential paths, per ADR#0023
/// Decision 3.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBaoChildTokens {
    role: OpenBaoTokenRole,
    policy: OpenBaoPolicyName,
}

impl OpenBaoChildTokens {
    pub fn new(role: OpenBaoTokenRole, policy: OpenBaoPolicyName) -> Self {
        Self { role, policy }
    }

    pub fn role(&self) -> &OpenBaoTokenRole {
        &self.role
    }

    pub fn policy(&self) -> &OpenBaoPolicyName {
        &self.policy
    }
}
