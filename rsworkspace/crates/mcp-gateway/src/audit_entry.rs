use crate::{ApprovalStatus, ServerName, ToolName, Verdict};

/// One gateway decision, kept for audit. Satisfies MCP-SECURITY-GATEWAY-1.0
/// MUST-18 ("every gateway decision produces an audit entry"), mirroring
/// AGT's `AuditEntry` dataclass (`agent_os/mcp_gateway.py`) as a value type
/// instead of a free-form dict.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    server_name: ServerName,
    tool_name: ToolName,
    verdict: Verdict,
    approval_status: Option<ApprovalStatus>,
    request_id: String,
    recorded_at_epoch_seconds: f64,
}

impl AuditEntry {
    pub fn new(
        server_name: ServerName,
        tool_name: ToolName,
        verdict: Verdict,
        approval_status: Option<ApprovalStatus>,
        request_id: impl Into<String>,
        recorded_at_epoch_seconds: f64,
    ) -> Self {
        Self {
            server_name,
            tool_name,
            verdict,
            approval_status,
            request_id: request_id.into(),
            recorded_at_epoch_seconds,
        }
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn verdict(&self) -> &Verdict {
        &self.verdict
    }

    pub fn approval_status(&self) -> Option<ApprovalStatus> {
        self.approval_status
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn recorded_at_epoch_seconds(&self) -> f64 {
        self.recorded_at_epoch_seconds
    }

    pub fn was_blocked(&self) -> bool {
        self.verdict.is_block()
    }
}

/// Sink for [`AuditEntry`] values. The gateway service must record every
/// decision (MUST-18); the default in-process sink used by tests and the
/// binary's initial cut is [`InMemoryAuditTrail`]. Production deployments
/// wanting durable storage can implement this trait against their own
/// backend without changing interception call sites.
pub trait AuditTrail: Send + Sync {
    fn record(&mut self, entry: AuditEntry);
}

/// In-memory [`AuditTrail`] that keeps every entry for the life of the
/// process. Intended for tests and as a starting point; it is not a
/// durable audit log.
#[derive(Debug, Default)]
pub struct InMemoryAuditTrail {
    entries: Vec<AuditEntry>,
}

impl InMemoryAuditTrail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }
}

impl AuditTrail for InMemoryAuditTrail {
    fn record(&mut self, entry: AuditEntry) {
        self.entries.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(verdict: Verdict) -> AuditEntry {
        AuditEntry::new(
            ServerName::new("web-tools").expect("valid"),
            ToolName::new("search").expect("valid"),
            verdict,
            Some(ApprovalStatus::Approved),
            "req-1",
            1000.0,
        )
    }

    #[test]
    fn exposes_all_constructed_fields() {
        let audit = entry(Verdict::Allow);
        assert_eq!(audit.server_name().as_str(), "web-tools");
        assert_eq!(audit.tool_name().as_str(), "search");
        assert_eq!(audit.verdict(), &Verdict::Allow);
        assert_eq!(audit.approval_status(), Some(ApprovalStatus::Approved));
        assert_eq!(audit.request_id(), "req-1");
        assert_eq!(audit.recorded_at_epoch_seconds(), 1000.0);
        assert!(!audit.was_blocked());
    }

    #[test]
    fn was_blocked_reflects_block_verdict() {
        let audit = entry(Verdict::Block {
            reason: "denied".to_string(),
            threats: Vec::new(),
        });
        assert!(audit.was_blocked());
    }

    #[test]
    fn in_memory_audit_trail_records_every_entry() {
        let mut trail = InMemoryAuditTrail::new();
        trail.record(entry(Verdict::Allow));
        trail.record(entry(Verdict::Flag { threats: Vec::new() }));
        assert_eq!(trail.entries().len(), 2);
    }
}
