use super::*;

/// [`SnapshotTagHistoryReader`] over the snapshot tag audit stream.
pub struct JetStreamSnapshotTagHistoryReader {
    jetstream: async_nats::jetstream::Context,
}

impl JetStreamSnapshotTagHistoryReader {
    pub fn new(jetstream: async_nats::jetstream::Context) -> Self {
        Self { jetstream }
    }
}

#[async_trait::async_trait]
impl SnapshotTagHistoryReader for JetStreamSnapshotTagHistoryReader {
    async fn read(
        &self,
        tenant: &TenantId,
        name: &SnapshotTagName,
    ) -> Result<Vec<SnapshotTagAuditEvent>, SnapshotTagHistoryError> {
        crate::snapshot_tag::read_tag_audit_history(self.jetstream.clone(), tenant, name)
            .await
            .map_err(|error| SnapshotTagHistoryError::Read {
                message: format!("{error:#}"),
            })
    }
}
