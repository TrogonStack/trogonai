use super::*;
use async_nats::jetstream::{kv, stream};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

#[derive(Clone, Copy, Debug)]
enum UnsafePolicy {
    Expiry,
    Eviction,
}

async fn unsafe_cleanup_storage_refuses_native_delete(policy: UnsafePolicy, already_open: bool) -> TestResult {
    let rig = Rig::new().await?;
    let context = async_nats::jetstream::new(rig.client.clone());
    let store = context
        .create_key_value(kv::Config {
            bucket: "sandbox-e2b-lease-cleanup".into(),
            history: 1,
            ..Default::default()
        })
        .await?;
    let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, &serde_json::to_string(&(SOURCE, "source"))?);
    store
        .create(
            key.clone(),
            serde_json::to_vec(&json!({
                "version":1,
                "tenant":TENANT,
                "lease_id":SOURCE,
                "sandbox_id":"source",
                "phase":"cleanup",
                "claim_until_unix_ms":0,
            }))?
            .into(),
        )
        .await?;
    rig.provider
        .lease(&sibling_request("tenant-b")?, &LeaseId::new("lease-sibling"))
        .await?;

    if already_open {
        rig.control.sandbox_delete_pending.store(true, Ordering::SeqCst);
        let pending = rig.provider.cancel(&LeaseId::new(SOURCE)).await;
        assert!(
            pending.is_err() && rig.control.is_active("source") && rig.control.delete_attempts() == ["source"],
            "opening the journal must leave exact unresolved custody when DELETE is acknowledged without absence: {pending:?}"
        );
        rig.control.sandbox_delete_pending.store(false, Ordering::SeqCst);
    }
    let before = store
        .entry(key.clone())
        .await?
        .ok_or("pending cleanup authority is missing")?;
    let attempts_before = rig.control.delete_attempts();
    let mut stream = store.stream.clone();
    let safe = stream.info().await?.config.clone();
    let mut unsafe_config = safe.clone();
    match policy {
        UnsafePolicy::Expiry => unsafe_config.max_age = Duration::from_secs(600),
        UnsafePolicy::Eviction => {
            unsafe_config.discard = stream::DiscardPolicy::Old;
            unsafe_config.max_messages = 1;
        }
    }
    context.update_stream(unsafe_config).await?;

    let refused = rig.provider.cancel(&LeaseId::new(SOURCE)).await;
    let attempts_after = rig.control.delete_attempts();
    let source_retained = rig.control.is_active("source");
    let sibling_retained = rig.control.is_active("other-1");
    let after = store.entry(key.clone()).await?;
    let authority_unchanged = after
        .as_ref()
        .is_some_and(|after| after.revision == before.revision && after.value == before.value);

    context.update_stream(safe).await?;
    rig.cancel(SOURCE).await?;
    let settled = store.get(key).await?.ok_or("settled cleanup receipt is missing")?;
    let settled: Value = serde_json::from_slice(&settled)?;
    let attempts_settled = rig.control.delete_attempts();
    let replay: Value = rig
        .call(
            "cancel_owned",
            SOURCE,
            json!({"tenant":TENANT, "lease_id":SOURCE, "sandbox_id":"source"}),
        )
        .await?;
    let replay_did_not_delete = rig.control.delete_attempts() == attempts_settled;
    let replay_settled =
        replay == json!({"outcome":"cleaned", "tenant":TENANT, "lease_id":SOURCE, "sandbox_id":"source"});
    rig.cancel("lease-sibling").await?;
    assert!(
        rig.control.no_active(),
        "owned native API fixtures must be absent before the policy assertion"
    );
    assert!(
        refused.is_err()
            && attempts_after == attempts_before
            && source_retained
            && sibling_retained
            && authority_unchanged
            && settled["phase"] == "absent"
            && replay_settled
            && replay_did_not_delete,
        "unsafe cleanup storage must refuse before DELETE and preserve exact receipt authority: policy={policy:?}, already_open={already_open}, refused={refused:?}, attempts_before={attempts_before:?}, attempts_after={attempts_after:?}, source_retained={source_retained}, sibling_retained={sibling_retained}, authority_unchanged={authority_unchanged}, settled={settled}, replay={replay}, replay_did_not_delete={replay_did_not_delete}"
    );
    Ok(())
}

#[tokio::test]
async fn existing_cleanup_bucket_expiry_refuses_native_delete() -> TestResult {
    unsafe_cleanup_storage_refuses_native_delete(UnsafePolicy::Expiry, false).await
}

#[tokio::test]
async fn cached_cleanup_bucket_expiry_refuses_native_delete() -> TestResult {
    unsafe_cleanup_storage_refuses_native_delete(UnsafePolicy::Expiry, true).await
}

#[tokio::test]
async fn existing_cleanup_bucket_eviction_refuses_native_delete() -> TestResult {
    unsafe_cleanup_storage_refuses_native_delete(UnsafePolicy::Eviction, false).await
}

#[tokio::test]
async fn cached_cleanup_bucket_eviction_refuses_native_delete() -> TestResult {
    unsafe_cleanup_storage_refuses_native_delete(UnsafePolicy::Eviction, true).await
}
