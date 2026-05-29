use crate::approvals::types::RequestId;

pub trait ApprovalAuditSink: Send + Sync {
    fn approval_requested(&self, request_id: &RequestId);
    fn approval_granted(&self, request_id: &RequestId, approver: &str);
    fn approval_denied(&self, request_id: &RequestId, approver: &str);
    fn approval_expired(&self, request_id: &RequestId);
}
