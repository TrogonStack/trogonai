use crate::audit_name::AuditName;

/// Who caused the act, by identifier only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    Unattributed,
    ApiKey { api_key_id: AuditName },
    HumanSession { subject: AuditName, issuer: AuditName },
    BreakGlass { operator: AuditName, host: AuditName },
    Agent { agent_id: AuditName },
}

impl Actor {
    pub fn is_break_glass(&self) -> bool {
        matches!(self, Self::BreakGlass { .. })
    }
}

#[cfg(test)]
mod tests;
