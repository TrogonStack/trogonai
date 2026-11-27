use super::*;

/// Always-empty [`SnapshotTagHistoryReader`] used as [`BrokerCore::new`]'s
/// default until [`BrokerCore::with_snapshot_tag_history_reader`] wires a real
/// one.
pub struct NoopSnapshotTagHistoryReader;

#[async_trait::async_trait]
impl SnapshotTagHistoryReader for NoopSnapshotTagHistoryReader {
    async fn read(
        &self,
        _tenant: &TenantId,
        _name: &SnapshotTagName,
    ) -> Result<Vec<SnapshotTagAuditEvent>, SnapshotTagHistoryError> {
        Ok(Vec::new())
    }
}
