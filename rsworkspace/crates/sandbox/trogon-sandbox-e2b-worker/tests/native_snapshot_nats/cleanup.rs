use super::*;
use trogon_sandbox_contract::NativeHandleResponse;
use trogon_sandbox_protocol::{AuthorityHost, HeldLease, PurgeOutcome};

fn owned(tenant: &str, lease: &str, sandbox: &str) -> Value {
    json!({"tenant":tenant, "lease_id":lease, "sandbox_id":sandbox})
}

fn cleaned() -> Value {
    json!({"outcome":"cleaned", "tenant":TENANT, "lease_id":SOURCE, "sandbox_id":"source"})
}

async fn cancel_owned_rpc(rig: &Rig, lease: &str, body: Value) -> TestResult<Value> {
    let reply = rig
        .client
        .send_request(
            format!("sandbox.provider.e2b.cancel_owned.{lease}"),
            async_nats::Request::new()
                .timeout(Some(Duration::from_millis(300)))
                .payload(serde_json::to_vec(&body)?.into()),
        )
        .await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

async fn is_held(rig: &Rig, lease: &str) -> TestResult<bool> {
    let held = rig.provider.held_leases().await?;
    Ok(held.iter().any(|held| held.lease_id().as_str() == lease))
}

async fn holds_cleanup(rig: &Rig, lease: &LeaseId, sandbox: &str) -> TestResult<bool> {
    let held = rig.provider.held_leases().await?;
    Ok(held.iter().any(|held| {
        matches!(held, HeldLease::Provider { lease_id, provider: ProviderKind::E2b, resource_id }
            if lease_id == lease && resource_id.as_str() == sandbox)
    }))
}

#[tokio::test]
async fn generic_cancel_retains_exact_absence_for_owned_replay_after_reconstruction() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.provider
        .lease(&sibling_request("tenant-b")?, &LeaseId::new("lease-sibling"))
        .await?;
    rig.control.observe_owned_cleanup_intent.store(true, Ordering::SeqCst);
    rig.cancel(SOURCE).await?;
    let intent_seen = rig.control.owned_cleanup_intent_seen.load(Ordering::SeqCst);
    let source_absent = !rig.control.is_active("source");
    let forgotten = !is_held(&rig, SOURCE).await?;
    let _reconstructed = rig.restart("owned-cleanup-reconstructed").await?;
    let replay = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "source")).await;
    let foreign = cancel_owned_rpc(&rig, SOURCE, owned("tenant-b", SOURCE, "source")).await;
    let different_resource = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "other-1")).await;
    let attempts = rig.control.delete_attempts();
    let sibling_preserved = rig.control.is_active("other-1");
    rig.cancel("lease-sibling").await?;
    assert!(
        rig.control.no_active(),
        "owned fixtures must be absent before assertion"
    );
    assert!(
        intent_seen
            && source_absent
            && forgotten
            && sibling_preserved
            && replay.as_ref().is_ok_and(|reply| *reply == cleaned())
            && foreign.is_err()
            && different_resource.is_err()
            && attempts == ["source"],
        "generic cancellation must retain exact durable authority before forgetting: intent_before_delete={intent_seen}, source_absent={source_absent}, forgotten={forgotten}, sibling_preserved={sibling_preserved}, replay={replay:?}, foreign={foreign:?}, different_resource={different_resource:?}, attempts={attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn generic_cancel_pending_native_deletion_recovers_through_owned_cleanup() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.control.observe_owned_cleanup_intent.store(true, Ordering::SeqCst);
    rig.control.sandbox_delete_pending.store(true, Ordering::SeqCst);
    let pending = rig.provider.cancel(&LeaseId::new(SOURCE)).await;
    let intent_seen = rig.control.owned_cleanup_intent_seen.load(Ordering::SeqCst);
    let source_retained = rig.control.is_active("source");
    let _reconstructed = rig.restart("owned-cleanup-reconstructed").await?;
    rig.control.sandbox_delete_pending.store(false, Ordering::SeqCst);
    let replay = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "source")).await;
    let absent = !rig.control.is_active("source");
    let attempts = rig.control.delete_attempts();
    rig.api()?.delete_sandbox("source").await?;
    assert!(
        rig.control.no_active(),
        "owned native fixture must be absent before assertion"
    );
    assert!(
        pending.is_err()
            && intent_seen
            && source_retained
            && absent
            && replay.as_ref().is_ok_and(|reply| *reply == cleaned())
            && attempts == ["source", "source"],
        "pending generic cancellation must retain exact cleanup authority across reconstruction: pending={pending:?}, intent_before_delete={intent_seen}, source_retained={source_retained}, absent={absent}, replay={replay:?}, attempts={attempts:?}"
    );
    Ok(())
}

async fn break_cleanup_journal(rig: &Rig) -> TestResult {
    async_nats::jetstream::new(rig.client.clone())
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "sandbox-e2b-lease-cleanup".into(),
            max_value_size: 1,
            history: 1,
            ..Default::default()
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn generic_cancel_refuses_before_delete_when_cleanup_journal_is_unavailable() -> TestResult {
    let rig = Rig::new().await?;
    break_cleanup_journal(&rig).await?;
    let refused = rig.provider.cancel(&LeaseId::new(SOURCE)).await;
    let attempts = rig.control.delete_attempts();
    let retained = rig.control.is_active("source");
    let cleanup_only = rig.provider.sandbox_for_lease(&LeaseId::new(SOURCE)).await.is_none();
    rig.api()?.delete_sandbox("source").await?;
    assert!(
        rig.control.no_active(),
        "owned native fixture must be absent before assertion"
    );
    assert!(
        refused.is_err() && attempts.is_empty() && retained && cleanup_only,
        "generic cancellation cannot bypass unavailable durable cleanup custody: refused={refused:?}, attempts={attempts:?}, retained={retained}, cleanup_only={cleanup_only}"
    );
    Ok(())
}

#[tokio::test]
async fn owned_cleanup_replays_durable_absence_after_lost_reply_and_provider_reconstruction() -> TestResult {
    let mut rig = Rig::new().await?;
    let body = owned(TENANT, SOURCE, "source");
    rig.client
        .publish_with_reply(
            format!("sandbox.provider.e2b.cancel_owned.{SOURCE}"),
            rig.client.new_inbox(),
            serde_json::to_vec(&body)?.into(),
        )
        .await?;
    rig.client.flush().await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while is_held(&rig, SOURCE).await? {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Boxed>(())
    })
    .await??;
    assert!(!rig.control.is_active("source"));
    let _reconstructed = rig.restart("owned-cleanup-reconstructed").await?;
    let replay = cancel_owned_rpc(&rig, SOURCE, body).await;
    let foreign = cancel_owned_rpc(&rig, SOURCE, owned("tenant-b", SOURCE, "source")).await;
    let wrong_id = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "other-native-id")).await;
    let attempts = rig.control.delete_attempts();
    assert!(
        rig.control.no_active(),
        "owned source absence must remain confirmed before assertion"
    );
    assert!(
        replay.as_ref().is_ok_and(|reply| *reply == cleaned())
            && foreign.is_err()
            && wrong_id.is_err()
            && attempts == ["source"],
        "lost cleanup reply must replay only the exact durable receipt after provider reconstruction: replay={replay:?}, foreign={foreign:?}, wrong_id={wrong_id:?}, attempts={attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn owned_cleanup_intent_precedes_delete_and_recovers_a_pending_native_resource() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.control.observe_owned_cleanup_intent.store(true, Ordering::SeqCst);
    rig.control.sandbox_delete_pending.store(true, Ordering::SeqCst);
    let body = owned(TENANT, SOURCE, "source");
    let pending = cancel_owned_rpc(&rig, SOURCE, body.clone()).await;
    let intent_seen = rig.control.owned_cleanup_intent_seen.load(Ordering::SeqCst);
    let retained_native = rig.control.is_active("source");
    let _reconstructed = rig.restart("owned-cleanup-reconstructed").await?;
    rig.control.sandbox_delete_pending.store(false, Ordering::SeqCst);
    let settled = cancel_owned_rpc(&rig, SOURCE, body).await;
    let absent = !rig.control.is_active("source");
    let attempts = rig.control.delete_attempts();
    rig.api()?.delete_sandbox("source").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        rig.control.no_active(),
        "owned native cleanup must settle before assertion"
    );
    assert!(
        pending.as_ref().is_ok_and(|reply| reply["outcome"] == "denied")
            && intent_seen
            && retained_native
            && settled.as_ref().is_ok_and(|reply| *reply == cleaned())
            && absent
            && attempts == ["source", "source"],
        "durable exact authority must precede DELETE and survive worker reconstruction: pending={pending:?}, intent_seen_before_delete={intent_seen}, retained_native={retained_native}, settled={settled:?}, absent={absent}, attempts={attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn unavailable_owned_cleanup_journal_never_grants_destructive_authority() -> TestResult {
    let rig = Rig::new().await?;
    break_cleanup_journal(&rig).await?;
    let reply = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "source")).await;
    let attempts = rig.control.delete_attempts();
    let retained = rig.control.is_active("source");
    rig.api()?.delete_sandbox("source").await?;
    assert!(rig.control.no_active(), "owned cleanup settled before assertion");
    assert!(
        reply.as_ref().is_ok_and(|reply| reply["outcome"] == "denied") && attempts.is_empty() && retained,
        "unavailable durable journal must refuse deletion before provider effects: reply={reply:?}, attempts={attempts:?}, retained={retained}"
    );
    Ok(())
}

#[tokio::test]
async fn owned_cancel_rpc_requires_exact_identity_and_acknowledges_only_confirmed_absence() -> TestResult {
    let rig = Rig::new().await?;
    rig.provider
        .lease(&sibling_request("tenant-b")?, &LeaseId::new("lease-sibling"))
        .await?;
    let foreign = cancel_owned_rpc(&rig, SOURCE, owned("tenant-b", SOURCE, "source")).await;
    let wrong_resource = cancel_owned_rpc(&rig, SOURCE, owned(TENANT, SOURCE, "other-1")).await;
    let invalid_never_deleted = rig.control.delete_attempts().is_empty();
    rig.control.sandbox_delete_pending.store(true, Ordering::SeqCst);
    let body = owned(TENANT, SOURCE, "source");
    let pending = cancel_owned_rpc(&rig, SOURCE, body.clone()).await;
    let retained = holds_cleanup(&rig, &LeaseId::new(SOURCE), "source").await?;
    let cleanup_only = rig.provider.sandbox_for_lease(&LeaseId::new(SOURCE)).await.is_none();
    rig.control.sandbox_delete_pending.store(false, Ordering::SeqCst);
    let settled = cancel_owned_rpc(&rig, SOURCE, body).await;
    let nonholder = cancel_owned_rpc(&rig, "lease-never-held", owned(TENANT, "lease-never-held", "source")).await;
    let nonholder_silent = nonholder.as_ref().is_err_and(|error| {
        error
            .downcast_ref::<async_nats::RequestError>()
            .is_some_and(|error| error.kind() == async_nats::RequestErrorKind::TimedOut)
    });
    let absent = !rig.control.is_active("source");
    let released = !is_held(&rig, SOURCE).await?;
    let sibling_untouched = rig.control.is_active("other-1")
        && rig
            .provider
            .sandbox_for_lease(&LeaseId::new("lease-sibling"))
            .await
            .is_some();
    let attempts = rig.control.delete_attempts();
    rig.api()?.delete_sandbox("source").await?;
    rig.cancel(SOURCE).await?;
    rig.cancel("lease-sibling").await?;
    assert!(
        rig.control.no_active(),
        "owned fixtures must be disposed before assertion"
    );
    let refused = |reply: &TestResult<Value>| {
        reply
            .as_ref()
            .is_ok_and(|reply| matches!(reply["outcome"].as_str(), Some("denied" | "not_held")))
    };
    assert!(
        refused(&foreign)
            && refused(&wrong_resource)
            && invalid_never_deleted
            && retained
            && cleanup_only
            && pending.as_ref().is_ok_and(|reply| reply["outcome"] == "denied")
            && settled.as_ref().is_ok_and(|reply| *reply == cleaned())
            && absent
            && released
            && sibling_untouched
            && nonholder_silent
            && attempts == ["source", "source"],
        "owned cleanup RPC must validate authority and retain unconfirmed custody: foreign={foreign:?}, wrong_resource={wrong_resource:?}, invalid_never_deleted={invalid_never_deleted}, pending={pending:?}, retained={retained}, cleanup_only={cleanup_only}, settled={settled:?}, absent={absent}, released={released}, sibling_untouched={sibling_untouched}, nonholder={nonholder:?}, attempts={attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn acknowledged_sandbox_delete_retains_exact_cleanup_custody_until_confirmed_absent() -> TestResult {
    let rig = Rig::new().await?;
    rig.provider
        .lease(&sibling_request("tenant-b")?, &LeaseId::new("lease-sibling"))
        .await?;
    rig.control.sandbox_delete_pending.store(true, Ordering::SeqCst);
    let source = LeaseId::new(SOURCE);
    let pending = rig.provider.cancel(&source).await;
    let retained = holds_cleanup(&rig, &source, "source").await?;
    let cleanup_only = rig.provider.sandbox_for_lease(&source).await.is_none()
        && matches!(
            rig.provider.native_handle_for_lease(&source).await,
            NativeHandleResponse::Denied(_)
        );
    let sibling_untouched = rig
        .provider
        .sandbox_for_lease(&LeaseId::new("lease-sibling"))
        .await
        .is_some()
        && rig.control.is_active("other-1")
        && rig.control.delete_attempts() == ["source"];
    rig.control.sandbox_delete_pending.store(false, Ordering::SeqCst);
    let retried = rig.provider.cancel(&source).await;
    let released = !is_held(&rig, SOURCE).await?;
    let absent = !rig.control.is_active("source");
    let attempts = rig.control.delete_attempts();
    rig.api()?.delete_sandbox("source").await?;
    rig.cancel("lease-sibling").await?;
    assert!(
        rig.control.no_active(),
        "owned fixtures must be disposed before assertion"
    );
    assert!(
        pending.is_err()
            && retained
            && cleanup_only
            && sibling_untouched
            && retried.is_ok()
            && released
            && absent
            && attempts == ["source", "source"],
        "DELETE acknowledgement must not retire visible native custody: pending={pending:?}, retained={retained}, cleanup_only={cleanup_only}, sibling_untouched={sibling_untouched}, retried={retried:?}, released={released}, absent={absent}, attempts={attempts:?}"
    );
    Ok(())
}

/// Captures a running source, then makes its restore fail the identity check while the fork cannot be deleted.
async fn rejected_restore(rig: &Rig, lease: &str) -> TestResult<TestResult<ProviderLease>> {
    rig.control.deadline_fixture.store(true, Ordering::SeqCst);
    let started: RunCommandResponse = rig
        .call(
            "run_command",
            SOURCE,
            json!({"lease_id":SOURCE,"command":["server"],"detached":true,"deadline":"lease"}),
        )
        .await?;
    assert!(matches!(started, RunCommandResponse::Started(_)));
    let receipt = native(&rig.capture().await?)?;
    rig.control.mismatch_restore.store(true, Ordering::SeqCst);
    rig.control.fail_restored_delete.store(true, Ordering::SeqCst);
    Ok(rig.restore(TENANT, lease, receipt.reference).await)
}

#[tokio::test]
async fn rejected_restore_with_failed_delete_retains_exact_cleanup_only_custody() -> TestResult {
    let rig = Rig::new().await?;
    let lease = LeaseId::new("rejected-restore");
    let restoration = rejected_restore(&rig, lease.as_str()).await?;
    let held_before = rig.provider.held_leases().await?;
    let exact_cleanup_held = holds_cleanup(&rig, &lease, "restored-1").await?;
    let inaccessible = rig.provider.sandbox_for_lease(&lease).await.is_none()
        && matches!(
            rig.provider.native_handle_for_lease(&lease).await,
            NativeHandleResponse::Denied(_)
        );
    let calls_before = rig.control.guest_calls.lock().len();
    let denied = rig
        .client
        .send_request(
            format!("sandbox.provider.e2b.run_command.{lease}"),
            async_nats::Request::new()
                .timeout(Some(Duration::from_millis(200)))
                .payload(
                    serde_json::to_vec(&json!({"lease_id":lease,"command":["must-not-run"],"detached":true}))?.into(),
                ),
        )
        .await;
    let no_guest_access = denied.is_err_and(|error| error.kind() == async_nats::RequestErrorKind::TimedOut)
        && rig.control.guest_calls.lock().len() == calls_before;
    let blocked_cancel = rig.provider.cancel(&lease).await;
    let held_after_retry = rig.provider.held_leases().await?;
    let retained = exact_cleanup_held && held_after_retry == held_before;
    rig.control.fail_restored_delete.store(false, Ordering::SeqCst);
    let retried_cancel = rig.provider.cancel(&lease).await;
    let disposed_by_retry = rig.control.process_running.lock().get("restored-1") == Some(&false);
    let released = !is_held(&rig, lease.as_str()).await?;
    let attempts = rig.control.delete_attempts();
    rig.api()?.delete_sandbox("restored-1").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        rig.control.all_stopped(),
        "owned rejected-restore fixtures must be disposed before assertion"
    );
    assert!(
        restoration.is_err()
            && inaccessible
            && no_guest_access
            && retained
            && blocked_cancel.is_err()
            && retried_cancel.is_ok()
            && disposed_by_retry
            && released
            && attempts == ["restored-1", "restored-1", "restored-1"],
        "rejected restore lost cleanup-only custody: restoration={restoration:?}, inaccessible={inaccessible}, no_guest_access={no_guest_access}, exact_cleanup_held={exact_cleanup_held}, retained={retained}, blocked_cancel={blocked_cancel:?}, retried_cancel={retried_cancel:?}, disposed_by_retry={disposed_by_retry}, released={released}, attempts={attempts:?}"
    );
    Ok(())
}

#[tokio::test]
async fn rejected_restore_cleanup_only_custody_is_retryable_by_authority_purge() -> TestResult {
    let rig = Rig::new().await?;
    let lease = LeaseId::new("purged-rejected-restore");
    let rejected = rejected_restore(&rig, lease.as_str()).await?;
    let held = rig
        .provider
        .held_leases()
        .await?
        .into_iter()
        .find(|held| matches!(held, HeldLease::Provider { lease_id, .. } if lease_id == &lease))
        .ok_or("the rejected restore left no cleanup custody")?;
    let blocked = AuthorityHost::purge(rig.provider.as_ref(), &held).await;
    let retained = rig.provider.held_leases().await?.contains(&held);
    rig.control.fail_restored_delete.store(false, Ordering::SeqCst);
    let purged = AuthorityHost::purge(rig.provider.as_ref(), &held).await;
    let disposed = rig.control.process_running.lock().get("restored-1") == Some(&false);
    let released = !is_held(&rig, lease.as_str()).await?;
    rig.api()?.delete_sandbox("restored-1").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        rig.control.all_stopped(),
        "owned authority purge fixtures must be disposed before assertion"
    );
    assert!(
        rejected.is_err()
            && blocked.is_err()
            && retained
            && matches!(purged, Ok(PurgeOutcome::Purged))
            && disposed
            && released,
        "authority purge must retry exact cleanup-only custody: rejected={rejected:?}, blocked={blocked:?}, retained={retained}, purged={purged:?}, disposed={disposed}, released={released}"
    );
    Ok(())
}
