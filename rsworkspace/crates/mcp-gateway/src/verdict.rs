use crate::McpThreat;

/// Typed outcome of running gateway interception (discovery scan, tool-call
/// pre-dispatch check, or tool-response post-dispatch check) against a
/// single piece of MCP traffic.
///
/// Unlike AGT's `MCPResponseDecision`, which carries a boolean `allowed`
/// plus a free-text `reason` that both interception stages had to agree on
/// by convention, `Verdict` makes the three outcomes an exhaustive enum so
/// callers cannot construct a `Flag` without threats or a `Block` without a
/// reason.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Traffic is clean (or scanning found nothing above policy
    /// threshold); forward unchanged.
    Allow,
    /// Traffic is denied outright; the gateway must not forward it.
    Block { reason: String, threats: Vec<McpThreat> },
    /// Traffic is forwarded but the detected threats are recorded for
    /// audit (matches AGT's `ResponsePolicy::Log` / `SANITIZE`-without-
    /// hard-block paths).
    Flag { threats: Vec<McpThreat> },
}

impl Verdict {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    pub fn is_flag(&self) -> bool {
        matches!(self, Self::Flag { .. })
    }

    /// Whether the gateway should still forward this traffic. `Allow` and
    /// `Flag` both forward; only `Block` stops the message.
    pub fn should_forward(&self) -> bool {
        !self.is_block()
    }

    pub fn threats(&self) -> &[McpThreat] {
        match self {
            Self::Allow => &[],
            Self::Block { threats, .. } | Self::Flag { threats } => threats,
        }
    }

    /// Build a verdict from a scan's threat findings and the configured
    /// [`crate::ResponsePolicy`]. `hard_block_types` names threat types
    /// that must block regardless of policy (matches AGT's
    /// `hard_block_categories` for credential/PII/exfiltration leaks,
    /// which `Sanitize` cannot safely strip from prose).
    pub fn from_threats(threats: Vec<McpThreat>, policy: crate::ResponsePolicy, reason: impl Into<String>) -> Self {
        if threats.is_empty() {
            return Self::Allow;
        }
        match policy {
            crate::ResponsePolicy::Log => Self::Flag { threats },
            crate::ResponsePolicy::Sanitize | crate::ResponsePolicy::Block => Self::Block {
                reason: reason.into(),
                threats,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{McpSeverity, McpThreatDetails, McpThreatType, ResponsePolicy, ServerName, ToolName};

    fn threat() -> McpThreat {
        McpThreat::new(
            McpThreatType::HiddenInstruction,
            McpSeverity::Critical,
            ToolName::new("search").expect("valid"),
            ServerName::new("web-tools").expect("valid"),
            "hidden instruction detected",
            None,
            McpThreatDetails::None,
        )
    }

    #[test]
    fn allow_forwards_and_has_no_threats() {
        let verdict = Verdict::Allow;
        assert!(verdict.is_allow());
        assert!(verdict.should_forward());
        assert!(verdict.threats().is_empty());
    }

    #[test]
    fn block_does_not_forward() {
        let verdict = Verdict::Block {
            reason: "denied".to_string(),
            threats: vec![threat()],
        };
        assert!(verdict.is_block());
        assert!(!verdict.should_forward());
        assert_eq!(verdict.threats().len(), 1);
    }

    #[test]
    fn flag_forwards_with_threats() {
        let verdict = Verdict::Flag {
            threats: vec![threat()],
        };
        assert!(verdict.is_flag());
        assert!(verdict.should_forward());
        assert_eq!(verdict.threats().len(), 1);
    }

    #[test]
    fn from_threats_allows_when_empty() {
        let verdict = Verdict::from_threats(Vec::new(), ResponsePolicy::Block, "n/a");
        assert_eq!(verdict, Verdict::Allow);
    }

    #[test]
    fn from_threats_blocks_under_block_policy() {
        let verdict = Verdict::from_threats(vec![threat()], ResponsePolicy::Block, "blocked");
        assert!(verdict.is_block());
    }

    #[test]
    fn from_threats_blocks_under_sanitize_policy_for_hard_block_findings() {
        let verdict = Verdict::from_threats(vec![threat()], ResponsePolicy::Sanitize, "blocked");
        assert!(verdict.is_block());
    }

    #[test]
    fn from_threats_flags_under_log_policy() {
        let verdict = Verdict::from_threats(vec![threat()], ResponsePolicy::Log, "n/a");
        assert!(verdict.is_flag());
    }
}
