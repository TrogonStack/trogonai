use super::*;

/// Read-only reader over a named snapshot tag's audit history (see
/// [`crate::snapshot_tag::read_tag_audit_history`]), which needs a live
/// `jetstream::Context` that [`BrokerCore`] does not otherwise hold. Follows
/// the same structural pattern as [`SnapshotKvReader`].
#[async_trait::async_trait]
pub trait SnapshotTagHistoryReader: Send + Sync {
    async fn read(
        &self,
        tenant: &TenantId,
        name: &SnapshotTagName,
    ) -> Result<Vec<SnapshotTagAuditEvent>, SnapshotTagHistoryError>;
}
