use super::*;
use crate::native_snapshot::NatsKvNativeSnapshotStore;
use crate::native_snapshot::{InMemoryNativeSnapshotStore, NativeSnapshotStore};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use trogon_sandbox_contract::{
    NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, NativeSnapshotId, NativeSnapshotRef,
};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

struct DeleteBus {
    calls: AtomicUsize,
    fail: AtomicBool,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for DeleteBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        assert_eq!(
            subject,
            trogon_sandbox_protocol::subjects::provider_native_snapshot_delete(ProviderKind::E2b)
        );
        let request: NativeSnapshotDeleteRequest = serde_json::from_slice(&payload)?;
        assert_eq!(request.reference.provider, ProviderKind::E2b);
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.fail.load(Ordering::SeqCst), "provider unavailable");
        Ok(serde_json::to_vec(&NativeSnapshotDeleteResponse::Deleted)?.into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

fn snapshot() -> anyhow::Result<NativeSnapshotRef> {
    Ok(NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new("opaque-capture")?,
        expires_at: None,
    })
}

fn deletion(tenant: &str) -> anyhow::Result<NativeSnapshotDeleteRequest> {
    Ok(NativeSnapshotDeleteRequest {
        tenant: TenantId::new(tenant)?,
        reference: snapshot()?,
    })
}

fn fixture() -> anyhow::Result<(BrokerCore, Arc<InMemoryNativeSnapshotStore>, Arc<DeleteBus>)> {
    let store = Arc::new(InMemoryNativeSnapshotStore::new());
    let bus = Arc::new(DeleteBus {
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    let core = make_core_with_bus(bus.clone())?.with_native_snapshot_store(store.clone());
    Ok((core, store, bus))
}

#[tokio::test]
async fn owned_native_snapshot_is_deleted_once_and_cannot_be_restored() -> TestResult {
    let (core, store, bus) = fixture()?;
    store.record(&TenantId::new("owner")?, &snapshot()?).await?;
    assert_eq!(
        core.handle_native_snapshot_delete(deletion("owner")?).await,
        NativeSnapshotDeleteResponse::Deleted
    );
    assert!(!store.owns(&TenantId::new("owner")?, &snapshot()?).await?);
    assert!(
        store.record(&TenantId::new("owner")?, &snapshot()?).await.is_err(),
        "a deleted identity must never be reactivated"
    );
    assert!(
        core.check_native_snapshot(&TenantId::new("owner")?, &snapshot()?, &[ProviderKind::E2b])
            .await
            .is_err()
    );
    assert_eq!(
        core.handle_native_snapshot_delete(deletion("owner")?).await,
        NativeSnapshotDeleteResponse::NotFound
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn foreign_and_unknown_native_snapshots_never_reach_a_provider() -> TestResult {
    let (core, store, bus) = fixture()?;
    store.record(&TenantId::new("owner")?, &snapshot()?).await?;
    assert_eq!(
        core.handle_native_snapshot_delete(deletion("other")?).await,
        NativeSnapshotDeleteResponse::NotFound
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 0);
    assert!(store.owns(&TenantId::new("owner")?, &snapshot()?).await?);
    Ok(())
}

#[tokio::test]
async fn failed_native_deletion_retains_intent_and_refuses_restore_until_retry() -> TestResult {
    let (core, store, bus) = fixture()?;
    store.record(&TenantId::new("owner")?, &snapshot()?).await?;
    bus.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        core.handle_native_snapshot_delete(deletion("owner")?).await,
        NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { .. })
    ));
    assert!(!store.owns(&TenantId::new("owner")?, &snapshot()?).await?);
    assert!(
        store.record(&TenantId::new("owner")?, &snapshot()?).await.is_err(),
        "capture retry must not erase durable deletion intent"
    );
    bus.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        core.handle_native_snapshot_delete(deletion("owner")?).await,
        NativeSnapshotDeleteResponse::Deleted
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn a_malformed_native_reference_cannot_be_constructed() {
    assert!(NativeSnapshotId::new("raw:default").is_err());
}

#[tokio::test]
async fn unsupported_provider_keeps_its_snapshot_restorable() -> TestResult {
    let (core, store, bus) = fixture()?;
    let mut request = deletion("owner")?;
    request.reference.provider = ProviderKind::Daytona;
    store.record(&request.tenant, &request.reference).await?;
    assert_eq!(
        core.handle_native_snapshot_delete(request.clone()).await,
        NativeSnapshotDeleteResponse::Unsupported
    );
    assert!(store.owns(&request.tenant, &request.reference).await?);
    assert_eq!(bus.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn expired_native_reference_can_still_be_retired() -> TestResult {
    let (core, store, bus) = fixture()?;
    let mut request = deletion("owner")?;
    request.reference.expires_at = Some(1);
    store.record(&request.tenant, &request.reference).await?;
    assert_eq!(
        core.handle_native_snapshot_delete(request).await,
        NativeSnapshotDeleteResponse::Deleted
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn native_snapshot_delete_intent_survives_a_broker_store_reopen() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(server.client.clone());
    let store = Arc::new(NatsKvNativeSnapshotStore::new(context.clone()).await?);
    let bus = Arc::new(DeleteBus {
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(true),
    });
    let request = deletion("owner")?;
    store.record(&request.tenant, &request.reference).await?;
    let core = make_core_with_bus(bus.clone())?.with_native_snapshot_store(store.clone());
    assert!(matches!(
        core.handle_native_snapshot_delete(request.clone()).await,
        NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { .. })
    ));
    drop(core);
    drop(store);
    let reopened = Arc::new(NatsKvNativeSnapshotStore::new(context).await?);
    assert!(!reopened.owns(&request.tenant, &request.reference).await?);
    assert!(reopened.record(&request.tenant, &request.reference).await.is_err());
    bus.fail.store(false, Ordering::SeqCst);
    let core = make_core_with_bus(bus.clone())?.with_native_snapshot_store(reopened.clone());
    assert_eq!(
        core.handle_native_snapshot_delete(request.clone()).await,
        NativeSnapshotDeleteResponse::Deleted
    );
    assert!(!reopened.owns(&request.tenant, &request.reference).await?);
    assert_eq!(
        core.handle_native_snapshot_delete(request).await,
        NativeSnapshotDeleteResponse::NotFound
    );
    assert_eq!(bus.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn native_snapshot_corrupt_records_and_kv_tombstones_grant_no_ownership() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(server.client.clone());
    let store = NatsKvNativeSnapshotStore::new(context.clone()).await?;
    let request = deletion("owner")?;
    let kv = context
        .get_key_value(trogon_sandbox_protocol::subjects::NATIVE_SNAPSHOT_KV_BUCKET)
        .await?;
    let key = tenant_scoped_kv_key(&request.tenant, &format!("e2b.{}", request.reference.id));
    kv.put(&key, Bytes::from_static(b"corrupt-owner-record")).await?;
    assert!(store.owns(&request.tenant, &request.reference).await.is_err());
    kv.delete(&key).await?;
    assert!(!store.owns(&request.tenant, &request.reference).await?);
    Ok(())
}

#[tokio::test]
async fn native_snapshot_unknown_legacy_fields_cannot_disguise_retired_or_foreign_records() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(server.client.clone());
    let store = NatsKvNativeSnapshotStore::new(context.clone()).await?;
    let request = deletion("owner")?;
    let kv = context
        .get_key_value(trogon_sandbox_protocol::subjects::NATIVE_SNAPSHOT_KV_BUCKET)
        .await?;
    let key = tenant_scoped_kv_key(&request.tenant, &format!("e2b.{}", request.reference.id));
    kv.put(
        &key,
        Bytes::from(serde_json::to_vec(&serde_json::json!({
            "provider":"e2b", "id":request.reference.id, "state":"deleted", "tenant":"foreign"
        }))?),
    )
    .await?;
    assert!(store.owns(&request.tenant, &request.reference).await.is_err());
    assert!(store.begin_delete(&request.tenant, &request.reference).await.is_err());
    Ok(())
}
