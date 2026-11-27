use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use chrono::{Datelike, Utc};
use trogon_sandbox_broker::quota::{NatsKvQuotaStore, QuotaRecord, QuotaStore, SnapshotQuota, quota_kv_key_for_month};
use trogon_sandbox_broker::snapshot_tracker::{InMemoryLiveSnapshotStore, account_snapshot_published};
use trogon_sandbox_contract::SnapshotReceipt;
use trogon_sandbox_contract::{
    ArtifactType, Digest, LeaseId, OciArtifactRef, SnapshotAccountingRefusal, SnapshotAccountingResponse, TenantId,
};

#[path = "../src/test_support.rs"]
mod test_support;

trait Accepted<T> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>>;
}

impl<T> Accepted<T> for Result<T, trogon_sandbox_contract::SnapshotAccountingRefusal> {
    fn accepted(self) -> Result<T, Box<dyn std::error::Error>> {
        self.map_err(|refusal| format!("snapshot accounting refused: {refusal:?}").into())
    }
}

async fn quota_handle(client: async_nats::Client) -> Result<Arc<NatsKvQuotaStore>, Box<dyn std::error::Error>> {
    Ok(Arc::new(
        NatsKvQuotaStore::new(jetstream::new(client), "legacy-snapshot-quarantine-proof").await?,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_legacy_usage_survives_monthly_expiry_and_fresh_broker_handles()
-> Result<(), Box<dyn std::error::Error>> {
    let server = test_support::LocalNats::start().await;
    let original = quota_handle(server.client.clone()).await?;
    let tenant = TenantId::new("legacy-expiry-tenant")?;
    let lease = LeaseId::new("legacy-expiry-capture");
    let store = original.snapshot_store().ok_or("no snapshot store")?;
    let reservation = store
        .reserve(&tenant, &lease, "legacy-expiry-capture")
        .await
        .accepted()?;
    let receipt = SnapshotReceipt {
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        bytes: 500,
    };
    store
        .bind(
            &reservation,
            OciArtifactRef {
                registry: "registry.example.test".into(),
                repository: trogon_sandbox_protocol::snapshot_repository_for_tenant(&tenant),
                digest: receipt.digest.clone(),
                artifact_type: receipt.artifact_type,
            },
            &receipt,
        )
        .await
        .accepted()?;
    store.seal(&reservation).await.accepted()?;
    let event = store.publication(&reservation).await.accepted()?;
    let mut legacy = QuotaRecord::new(3600, 7200);
    legacy.snapshot = SnapshotQuota::new(7, 7_000);
    legacy.snapshot.count_used = 2;
    legacy.snapshot.bytes_used = 1_000;
    let now = Utc::now();
    let key = quota_kv_key_for_month(&tenant, now.year(), now.month());
    original.kv.put(&key, serde_json::to_vec(&legacy)?.into()).await?;
    let live = InMemoryLiveSnapshotStore::new();
    assert!(matches!(
        account_snapshot_published(
            live.as_ref(),
            original.as_ref(),
            SnapshotQuota::new(100, 1_000_000),
            &event
        )
        .await,
        SnapshotAccountingResponse::Refused {
            reason: SnapshotAccountingRefusal::LegacyUnconfirmed
        }
    ));
    assert!(original.quota_record(&tenant).await.is_err());
    let mut config = original.kv.stream.cached_info().config.clone();
    config.max_age = Duration::from_millis(100);
    config.duplicate_window = Duration::from_millis(100);
    jetstream::new(server.client.clone()).update_stream(config).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while original.kv.entry(&key).await?.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), async_nats::jetstream::kv::EntryError>(())
    })
    .await??;
    drop(store);
    drop(original);
    let reopened = quota_handle(server.fresh_client().await).await?;
    let response = account_snapshot_published(
        live.as_ref(),
        reopened.as_ref(),
        SnapshotQuota::new(100, 1_000_000),
        &event,
    )
    .await;
    assert!(
        matches!(
            response,
            SnapshotAccountingResponse::Refused {
                reason: SnapshotAccountingRefusal::LegacyUnconfirmed
            }
        ),
        "expiry and fresh broker handles must not erase an observed unresolved legacy charge or admit defaults: {response:?}"
    );
    let preserved = reopened
        .legacy_snapshot_quota(&tenant)
        .await?
        .ok_or("no legacy quota")?;
    assert_eq!(
        (
            preserved.count_used,
            preserved.bytes_used,
            preserved.count_cap,
            preserved.bytes_cap
        ),
        (2, 1_000, 7, 7_000)
    );
    assert!(
        reopened.quota_record(&tenant).await.is_err(),
        "unconfirmed legacy usage cannot become an ordinary default quota record"
    );
    Ok(())
}
