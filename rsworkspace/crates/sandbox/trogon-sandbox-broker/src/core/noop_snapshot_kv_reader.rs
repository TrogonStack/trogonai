use super::*;

/// Always-`NotFound` [`SnapshotKvReader`] used as [`BrokerCore::new`]'s
/// default until [`BrokerCore::with_snapshot_kv_reader`] wires a real one.
pub struct NoopSnapshotKvReader;

#[async_trait::async_trait]
impl SnapshotKvReader for NoopSnapshotKvReader {
    async fn get(&self, _digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        Ok(None)
    }
}
