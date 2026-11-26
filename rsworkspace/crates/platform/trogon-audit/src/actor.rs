use crate::audit_name::AuditName;

/// Who caused the act, by identifier only. A platform service acting under its
/// bus principal is a `Service`, never an `Agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Unattributed,
    ApiKey { api_key_id: AuditName },
    HumanSession { subject: AuditName, issuer: AuditName },
    BreakGlass { operator: AuditName, host: AuditName },
    Agent { agent_id: AuditName },
    Service { service_id: AuditName },
}

impl Actor {
    pub fn is_break_glass(&self) -> bool {
        matches!(self, Self::BreakGlass { .. })
    }
}

#[cfg(test)]
mod tests;
