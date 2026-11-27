use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::SnapshotTagAuditEvent;

use super::snapshot_tag_audit_error::SnapshotTagAuditError;
use super::snapshot_tag_auditor::SnapshotTagAuditor;

pub struct InMemorySnapshotTagAuditor {
    events: Mutex<Vec<SnapshotTagAuditEvent>>,
}

impl InMemorySnapshotTagAuditor {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    pub async fn events(&self) -> Vec<SnapshotTagAuditEvent> {
        self.events.lock().await.clone()
    }
}

impl Default for InMemorySnapshotTagAuditor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SnapshotTagAuditor for InMemorySnapshotTagAuditor {
    async fn emit(&self, event: SnapshotTagAuditEvent) -> Result<(), SnapshotTagAuditError> {
        self.events.lock().await.push(event);
        Ok(())
    }
}
