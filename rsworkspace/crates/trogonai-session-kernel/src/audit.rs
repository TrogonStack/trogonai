//! §1887 Auditoría completa de tool calls.
//!
//! The event log + canonical snapshot make tool calls auditable (§291 "auditar que paso...
//! debuggear tool calls"). This module produces a complete, no-lossy audit of the canonical
//! tool calls of a session over the specified `CanonicalToolCall` contract (§827-851):
//! every call's name, full input JSON, status, error and lineage are preserved (§2200 — tool
//! IO is canonical truth, never truncated for the audit), and the calls are summarized by
//! status with the safety-relevant `requires_reconciliation` set called out (§ Failure Mode
//! Policy). Pure + deterministic.

use trogonai_session_contracts::{CanonicalToolCall, SessionSnapshotState, ToolCallStatus};

/// One audited tool call, preserving the canonical fields (no-lossy, §2200).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallAuditEntry {
    pub id: String,
    pub tool_execution_id: String,
    pub name: String,
    pub status: &'static str,
    pub input_json: String,
    pub error: Option<String>,
    pub parent_tool_use_id: Option<String>,
    pub has_result: bool,
}

/// Complete audit of a session's canonical tool calls (§1887).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolCallAudit {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub pending: usize,
    pub requires_reconciliation: usize,
    /// Ids of tool calls left in `requires_reconciliation` (§ Failure Mode Policy: not safe
    /// to auto-re-run; a switch/cancel must surface them).
    pub reconciliation_ids: Vec<String>,
    pub entries: Vec<ToolCallAuditEntry>,
}

/// Stable label for a tool call status (§827 `status`).
pub fn tool_call_status_label(status: Option<ToolCallStatus>) -> &'static str {
    match status {
        Some(ToolCallStatus::Pending) => "pending",
        Some(ToolCallStatus::Started) => "started",
        Some(ToolCallStatus::Completed) => "completed",
        Some(ToolCallStatus::Failed) => "failed",
        Some(ToolCallStatus::RequiresReconciliation) => "requires_reconciliation",
        Some(ToolCallStatus::Unspecified) | None => "unspecified",
    }
}

/// Audit the canonical tool calls of a materialized session snapshot (§1887).
pub fn audit_tool_calls(state: &SessionSnapshotState) -> ToolCallAudit {
    audit_from_calls(&state.tool_calls)
}

/// Audit a slice of canonical tool calls directly (used by the snapshot audit and tests).
pub fn audit_from_calls(calls: &[CanonicalToolCall]) -> ToolCallAudit {
    let mut audit = ToolCallAudit {
        total: calls.len(),
        ..ToolCallAudit::default()
    };
    for call in calls {
        let known = call.status.as_known();
        match known {
            Some(ToolCallStatus::Completed) => audit.completed += 1,
            Some(ToolCallStatus::Failed) => audit.failed += 1,
            Some(ToolCallStatus::Pending) | Some(ToolCallStatus::Started) => audit.pending += 1,
            Some(ToolCallStatus::RequiresReconciliation) => {
                audit.requires_reconciliation += 1;
                audit.reconciliation_ids.push(call.id.clone());
            }
            _ => {}
        }
        audit.entries.push(ToolCallAuditEntry {
            id: call.id.clone(),
            tool_execution_id: call.tool_execution_id.clone(),
            name: call.name.clone(),
            status: tool_call_status_label(known),
            input_json: call.input_json.clone(),
            error: call.error.clone(),
            parent_tool_use_id: call.parent_tool_use_id.clone(),
            has_result: call.result.as_option().is_some(),
        });
    }
    audit
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::{EnumValue, MessageField};
    use trogonai_session_contracts::{TextToolResult, ToolCallResult};

    fn call(id: &str, name: &str, status: ToolCallStatus, error: Option<&str>, with_result: bool) -> CanonicalToolCall {
        CanonicalToolCall {
            id: id.to_string(),
            tool_execution_id: format!("texec_{id}"),
            name: name.to_string(),
            input_json: format!("{{\"for\":\"{id}\"}}"),
            status: EnumValue::Known(status),
            error: error.map(str::to_string),
            result: if with_result {
                MessageField::some(ToolCallResult {
                    kind: Some(
                        TextToolResult {
                            content: "ok".to_string(),
                            ..TextToolResult::default()
                        }
                        .into(),
                    ),
                    ..ToolCallResult::default()
                })
            } else {
                MessageField::none()
            },
            ..CanonicalToolCall::default()
        }
    }

    #[test]
    fn audit_counts_by_status_and_preserves_canonical_fields() {
        let calls = vec![
            call("t1", "fs_read", ToolCallStatus::Completed, None, true),
            call("t2", "fs_write", ToolCallStatus::Failed, Some("disk full"), false),
            call("t3", "bash", ToolCallStatus::RequiresReconciliation, None, false),
        ];
        let audit = audit_from_calls(&calls);
        assert_eq!(audit.total, 3);
        assert_eq!(audit.completed, 1);
        assert_eq!(audit.failed, 1);
        assert_eq!(audit.requires_reconciliation, 1);
        assert_eq!(audit.reconciliation_ids, vec!["t3".to_string()]);
        // No-lossy: name, full input and error are preserved in the audit entry.
        let t2 = audit.entries.iter().find(|e| e.id == "t2").unwrap();
        assert_eq!(t2.name, "fs_write");
        assert_eq!(t2.status, "failed");
        assert_eq!(t2.error.as_deref(), Some("disk full"));
        assert!(t2.input_json.contains("t2"));
        assert!(!t2.has_result);
        let t1 = audit.entries.iter().find(|e| e.id == "t1").unwrap();
        assert!(t1.has_result);
    }

    #[test]
    fn empty_session_audits_to_zero() {
        let audit = audit_tool_calls(&SessionSnapshotState::default());
        assert_eq!(audit.total, 0);
        assert!(audit.entries.is_empty());
        assert!(audit.reconciliation_ids.is_empty());
    }
}
