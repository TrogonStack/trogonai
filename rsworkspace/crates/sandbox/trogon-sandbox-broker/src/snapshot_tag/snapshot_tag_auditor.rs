use async_trait::async_trait;
use trogon_sandbox_contract::SnapshotTagAuditEvent;

use super::snapshot_tag_audit_error::SnapshotTagAuditError;

#[async_trait]
pub trait SnapshotTagAuditor: Send + Sync {
    async fn emit(&self, event: SnapshotTagAuditEvent) -> Result<(), SnapshotTagAuditError>;
}
