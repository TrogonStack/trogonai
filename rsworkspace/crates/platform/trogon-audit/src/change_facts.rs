use crate::audit_name::AuditName;
use crate::change_terminal::ChangeTerminal;

/// A policy or credential change, pointing at the event it was projected from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeFacts {
    pub kind: ChangeTerminal,
    pub resource_name: AuditName,
    pub event_type: AuditName,
    pub source_stream: AuditName,
    pub stream_sequence: u64,
    pub event_id: AuditName,
}
