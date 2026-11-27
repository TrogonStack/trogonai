use super::lock::lock;
use super::*;

/// In-memory [`SnapshotTagHistoryReader`] fed by [`Self::push`].
#[derive(Default)]
pub struct InMemorySnapshotTagHistoryReader {
    events: std::sync::Mutex<Vec<SnapshotTagAuditEvent>>,
}

impl InMemorySnapshotTagHistoryReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, event: SnapshotTagAuditEvent) {
        lock(&self.events).push(event);
    }
}

#[async_trait::async_trait]
impl SnapshotTagHistoryReader for InMemorySnapshotTagHistoryReader {
    async fn read(
        &self,
        tenant: &TenantId,
        name: &SnapshotTagName,
    ) -> Result<Vec<SnapshotTagAuditEvent>, SnapshotTagHistoryError> {
        Ok(lock(&self.events)
            .iter()
            .filter(|event| match event {
                SnapshotTagAuditEvent::Set { tag, .. } | SnapshotTagAuditEvent::Cleared { tag, .. } => {
                    &tag.tenant == tenant && tag.name == *name
                }
            })
            .cloned()
            .collect())
    }
}
