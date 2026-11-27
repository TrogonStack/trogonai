use trogon_sandbox_contract::TenantId;

use super::refusal_result::Result;
use super::snapshot_accounting_store::SnapshotAccountingStore;
use crate::quota::SnapshotQuota;
use trogon_sandbox_contract::SnapshotAccountingRefusal as Refusal;

impl SnapshotAccountingStore {
    pub async fn usage(&self, tenant: &TenantId) -> Result<Option<SnapshotQuota>> {
        let Some(row) = self.read(tenant).await? else {
            return Ok(None);
        };
        if row.unconfirmed_legacy.is_some() {
            return Err(Refusal::LegacyUnconfirmed);
        }
        Ok(row.quota)
    }

    pub async fn legacy_observation(&self, tenant: &TenantId) -> Result<Option<SnapshotQuota>> {
        Ok(self.read(tenant).await?.and_then(|row| row.unconfirmed_legacy))
    }

    pub async fn quarantine_legacy(&self, tenant: &TenantId, observed: &SnapshotQuota) -> Result<()> {
        if observed.count_used == 0 && observed.bytes_used == 0 {
            return Ok(());
        }
        self.mutate(tenant, |row| {
            row.unconfirmed_legacy.get_or_insert_with(|| observed.clone());
            Ok(())
        })
        .await
    }
}
