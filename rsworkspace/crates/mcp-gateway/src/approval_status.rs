use std::fmt;

/// Result of a human-approval check for a sensitive tool call.
///
/// Exactly three variants per MCP-SECURITY-GATEWAY-1.0 MUST-3, mirroring
/// AGT's `ApprovalStatus` enum (`agent_os/mcp_gateway.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
}

impl fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_exactly_three_variants_with_stable_names() {
        assert_eq!(ApprovalStatus::Pending.as_str(), "pending");
        assert_eq!(ApprovalStatus::Approved.as_str(), "approved");
        assert_eq!(ApprovalStatus::Denied.as_str(), "denied");
    }
}
