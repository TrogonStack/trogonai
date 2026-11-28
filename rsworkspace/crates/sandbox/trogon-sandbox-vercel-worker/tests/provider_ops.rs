mod provider_support;
mod support;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use provider_support::*;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    LeaseError, NativeHandleResponse, ProviderKind, SandboxFreezeResponse, SnapshotExportError, SnapshotExportOutcome,
};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, HeldLease, Provider, PurgeOutcome,
};
use trogon_sandbox_vercel_worker::VercelWorkerProvider;

const SESSION: &str = "/v2/sandboxes/sessions/sess_1/stop";
const SNAPSHOT: &str = "/v3/sandboxes/sessions/sess_1/snapshot";
const SANDBOX: &str = "/v2/sandboxes/sbx-a";

fn error_body(message: &str) -> Value {
    json!({"error": {"message": message}})
}

fn snapshot_body() -> Value {
    json!({
        "session": {"id": "sess_1"},
        "snapshot": {"id": "snap_9", "status": "created", "sizeBytes": 42, "expiresAt": 1_750_344_501_629u64}
    })
}

async fn leased() -> Result<(Fake, VercelWorkerProvider), Boxed> {
    let fake = Fake::start().await?;
    fake.created();
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    Ok((fake, provider))
}

fn held(lease: &str, name: &str, kind: ProviderKind) -> Result<HeldLease, Boxed> {
    Ok(HeldLease::Provider {
        lease_id: lease_id(lease),
        provider: kind,
        resource_id: AuthorityResourceId::new(name)?,
    })
}

#[tokio::test]
async fn cancel_deletes_the_sandbox_and_is_idempotent() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("DELETE", SANDBOX, 200, json!({}));
    provider.cancel(&lease_id("l1")).await?;
    provider.cancel(&lease_id("l1")).await?;
    assert_eq!(fake.count("DELETE", SANDBOX), 1);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_cancel_retains_the_resource_until_cleanup_is_confirmed() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("DELETE", SANDBOX, 503, error_body("retry cleanup"));
    assert!(provider.cancel(&lease_id("l1")).await.is_err());
    assert_eq!(provider.active_lease_count().await, 1);
    fake.on("DELETE", SANDBOX, 200, json!({}));
    provider.cancel(&lease_id("l1")).await?;
    assert_eq!(provider.active_lease_count().await, 0);
    provider.cancel(&lease_id("l1")).await?;
    assert_eq!(fake.count("DELETE", SANDBOX), 2);
    Ok(())
}

#[tokio::test]
async fn pause_stops_the_session_and_resume_adopts_the_new_one() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("POST", SESSION, 200, json!({"session": {"id": "sess_1"}}));
    fake.on("GET", SANDBOX, 200, record("sess_2", "https://two.example.test"));
    let id = lease_id("l1");
    assert_eq!(
        provider.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    );
    assert_eq!(fake.count("POST", SESSION), 1);
    assert_eq!(
        provider.set_paused_for_lease(&id, false).await,
        SandboxFreezeResponse::Done
    );
    let resumes = fake.calls("GET", SANDBOX);
    assert!(resumes.iter().all(|call| call.path.contains("resume=true")));
    let sandbox = provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(sandbox.session_id, "sess_2");
    assert_eq!(
        sandbox.routes.get(&3000).map(String::as_str),
        Some("https://two.example.test")
    );
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("nope"), true).await,
        SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn stopping_twice_is_idempotent_and_other_failures_are_denied() -> TestResult {
    let (fake, provider) = leased().await?;
    let id = lease_id("l1");
    fake.on("POST", SESSION, 409, error_body("already stopped"));
    assert_eq!(
        provider.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Done
    );
    fake.on("POST", SESSION, 500, error_body("boom"));
    assert!(matches!(
        provider.set_paused_for_lease(&id, true).await,
        SandboxFreezeResponse::Denied(LeaseError::Backend { .. })
    ));
    fake.on("GET", SANDBOX, 503, error_body("later"));
    assert!(matches!(
        provider.set_paused_for_lease(&id, false).await,
        SandboxFreezeResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_non_persistent_sandbox_refuses_to_pause() -> TestResult {
    let fake = Fake::start().await?;
    fake.on(
        "POST",
        "/v2/sandboxes",
        200,
        json!({"sandbox": {"name": "sbx-a", "persistent": false}, "session": {"id": "sess_1"}}),
    );
    fake.on("POST", SESSION, 200, json!({}));
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), true).await,
        SandboxFreezeResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    assert_eq!(fake.count("POST", SESSION), 0);
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_returns_a_receipt_with_expiry_in_seconds() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("POST", SNAPSHOT, 201, snapshot_body());
    let SnapshotExportOutcome::Native(receipt) = provider.native_snapshot("sess_1").await else {
        return Err("expected a native receipt".into());
    };
    assert_eq!(receipt.reference.id.as_str(), "snap_9");
    assert_eq!(receipt.reference.provider, ProviderKind::Vercel);
    assert_eq!(receipt.reference.expires_at, Some(1_750_344_501));
    assert_eq!(receipt.bytes, Some(42));
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_failure_is_typed_by_what_a_retry_can_change() -> TestResult {
    let (fake, provider) = leased().await?;
    let failed = json!({"snapshot": {"id": "snap_f", "status": "failed"}});
    let unnamed = json!({"snapshot": {"id": "not a valid id", "status": "created"}});
    let unreported = json!({"snapshot": {"id": "snap_u"}});
    let deleted = json!({"snapshot": {"id": "snap_d", "status": "deleted"}});
    let cases = [
        (404, error_body("no"), true),
        (401, error_body("no"), true),
        (400, error_body("no"), true),
        (200, failed, true),
        (200, unnamed, true),
        (200, unreported, true),
        (200, deleted, true),
        (503, error_body("later"), false),
        (409, error_body("busy"), false),
        (429, error_body("slow"), false),
    ];
    for (status, body, permanent) in cases {
        fake.on("POST", SNAPSHOT, status, body);
        let outcome = provider.native_snapshot("sess_1").await;
        let SnapshotExportOutcome::Err(error) = outcome else {
            return Err("expected a refusal".into());
        };
        assert_eq!(matches!(error, SnapshotExportError::Permanent { .. }), permanent);
    }
    let unsafe_session = provider.native_snapshot("../x").await;
    assert!(matches!(
        unsafe_session,
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_snapshot_resumes_onto_a_new_session() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("POST", SNAPSHOT, 201, snapshot_body());
    fake.on("GET", SANDBOX, 200, record("sess_2", "https://two.example.test"));
    let id = lease_id("l1");
    assert!(matches!(
        provider.snapshot_lease(&id).await,
        SnapshotExportOutcome::Native(_)
    ));
    assert_eq!(fake.count("POST", SNAPSHOT), 1);
    let sandbox = provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(sandbox.session_id, "sess_2");
    assert_eq!(
        sandbox.routes.get(&3000).map(String::as_str),
        Some("https://two.example.test")
    );
    assert_eq!(fake.count("GET", SANDBOX), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_resume_keeps_the_receipt_and_the_next_operation_retries() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("POST", SNAPSHOT, 201, snapshot_body());
    fake.on("GET", SANDBOX, 503, error_body("later"));
    let id = lease_id("l1");
    assert!(matches!(
        provider.snapshot_lease(&id).await,
        SnapshotExportOutcome::Native(_)
    ));
    assert_eq!(fake.count("GET", SANDBOX), 1);

    // The retry fails too, and the lease still resolves to its last known session.
    let stale = provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(stale.session_id, "sess_1");
    assert_eq!(fake.count("GET", SANDBOX), 2);

    fake.on("GET", SANDBOX, 200, record("sess_2", "https://two.example.test"));
    let adopted = provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(adopted.session_id, "sess_2");
    assert_eq!(fake.count("GET", SANDBOX), 3);
    provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(fake.count("GET", SANDBOX), 3);
    Ok(())
}

#[tokio::test]
async fn a_failed_snapshot_leaves_the_session_alone() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("POST", SNAPSHOT, 503, error_body("later"));
    let id = lease_id("l1");
    assert!(matches!(
        provider.snapshot_lease(&id).await,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    let sandbox = provider.sandbox_for_lease(&id).await.ok_or("lease not held")?;
    assert_eq!(sandbox.session_id, "sess_1");
    assert_eq!(fake.count("GET", SANDBOX), 0);
    Ok(())
}

#[tokio::test]
async fn snapshotting_an_unknown_lease_is_permanent() -> TestResult {
    let fake = Fake::start().await?;
    assert!(matches!(
        fake.provider()?.snapshot_lease(&lease_id("nope")).await,
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { .. })
    ));
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_operation_during_a_snapshot_waits_and_sees_the_new_session() -> TestResult {
    let (taking, taken) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let taking = Mutex::new(taking);
    let released = Mutex::new(released);
    let fake = Fake::start_with(move |request| {
        if request.route() == SNAPSHOT {
            let _ = taking.lock().map(|sender| sender.send(()));
            let _ = released
                .lock()
                .map(|receiver| receiver.recv_timeout(std::time::Duration::from_secs(10)));
        }
    })
    .await?;
    fake.created();
    fake.on("POST", SNAPSHOT, 201, snapshot_body());
    fake.on("GET", SANDBOX, 200, record("sess_2", "https://two.example.test"));
    let provider = Arc::new(fake.provider()?);
    let id = lease_id("l1");
    provider.lease(&request()?, &id).await?;

    let snapper = {
        let (provider, id) = (provider.clone(), id.clone());
        tokio::spawn(async move { provider.snapshot_lease(&id).await })
    };
    tokio::task::spawn_blocking(move || taken.recv_timeout(std::time::Duration::from_secs(10))).await??;
    let waiter = {
        let (provider, id) = (provider.clone(), id.clone());
        tokio::spawn(async move { provider.sandbox_for_lease(&id).await })
    };
    release.send(())?;
    let sandbox = within(waiter).await??.ok_or("lease not held")?;
    assert_eq!(sandbox.session_id, "sess_2");
    assert!(matches!(within(snapper).await??, SnapshotExportOutcome::Native(_)));
    Ok(())
}

#[tokio::test]
async fn the_native_handle_names_the_sandbox() -> TestResult {
    let (_fake, provider) = leased().await?;
    let NativeHandleResponse::Available(handle) = provider.native_handle_for_lease(&lease_id("l1")).await else {
        return Err("no handle".into());
    };
    assert_eq!(handle.provider, ProviderKind::Vercel);
    assert_eq!(handle.vendor_id, "sbx-a");
    assert_eq!(handle.region.as_deref(), Some("iad1"));
    assert!(matches!(
        provider.native_handle_for_lease(&lease_id("x")).await,
        NativeHandleResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn held_leases_lists_each_sandbox_by_name() -> TestResult {
    let (_fake, provider) = leased().await?;
    let leases = provider.held_leases().await?;
    assert_eq!(leases.len(), 1);
    let lease = leases.first().ok_or("nothing held")?;
    assert_eq!(lease.lease_id(), &lease_id("l1"));
    assert!(matches!(
        lease,
        HeldLease::Provider { provider: ProviderKind::Vercel, resource_id, .. } if resource_id.as_str() == "sbx-a"
    ));
    Ok(())
}

#[tokio::test]
async fn purging_the_exact_resource_deletes_it() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("DELETE", SANDBOX, 200, json!({}));
    let outcome = AuthorityHost::purge(&provider, &held("l1", "sbx-a", ProviderKind::Vercel)?).await?;
    assert_eq!(outcome, PurgeOutcome::Purged);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn purging_a_resource_that_is_already_gone_is_settled() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("DELETE", SANDBOX, 404, error_body("gone"));
    let outcome = AuthorityHost::purge(&provider, &held("l1", "sbx-a", ProviderKind::Vercel)?).await?;
    assert_eq!(outcome, PurgeOutcome::Settled);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_exact_purge_keeps_the_lease() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("DELETE", SANDBOX, 503, error_body("later"));
    let failed = AuthorityHost::purge(&provider, &held("l1", "sbx-a", ProviderKind::Vercel)?).await;
    assert!(matches!(failed, Err(AuthorityHostError::Failed(_))));
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn purging_an_identity_the_worker_does_not_hold_inspects_the_resource() -> TestResult {
    let (fake, provider) = leased().await?;
    fake.on("GET", "/v2/sandboxes/other", 404, error_body("gone"));
    fake.on("GET", "/v2/sandboxes/live", 200, record("sess_9", "https://x.test"));
    fake.on("GET", "/v2/sandboxes/broken", 503, error_body("later"));

    let gone = AuthorityHost::purge(&provider, &held("l2", "other", ProviderKind::Vercel)?).await?;
    assert_eq!(gone, PurgeOutcome::Settled);
    let live = AuthorityHost::purge(&provider, &held("l2", "live", ProviderKind::Vercel)?).await?;
    assert_eq!(live, PurgeOutcome::Stale);
    // Held under the same lease id but a different resource name.
    let renamed = AuthorityHost::purge(&provider, &held("l1", "live", ProviderKind::Vercel)?).await?;
    assert_eq!(renamed, PurgeOutcome::Stale);
    let broken = AuthorityHost::purge(&provider, &held("l2", "broken", ProviderKind::Vercel)?).await;
    assert!(matches!(broken, Err(AuthorityHostError::Failed(_))));
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn purging_another_providers_identity_is_stale_without_a_call() -> TestResult {
    let fake = Fake::start().await?;
    let provider = fake.provider()?;
    let outcome = AuthorityHost::purge(&provider, &held("l1", "sbx-a", ProviderKind::Fly)?).await?;
    assert_eq!(outcome, PurgeOutcome::Stale);
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test]
async fn a_snapshot_taken_from_a_managed_image_restores_into_an_independent_sandbox() -> TestResult {
    let fake = Fake::start().await?;
    fake.on(
        "POST",
        "/v3/sandboxes",
        200,
        record("sess_1", "https://one.example.test"),
    );
    fake.on("POST", SNAPSHOT, 201, snapshot_body());
    fake.on("GET", SANDBOX, 200, record("sess_2", "https://two.example.test"));
    let provider = fake.provider()?;
    let managed = request_with(|wire| {
        wire["spec"]["image"]["reference"]["repository"] = json!("vercel/sandbox/node");
        wire["spec"]["image"]["reference"]["tag"] = json!("24");
        wire["native"] = json!({"vercel": {"persistent": true}});
    })?;
    provider.lease(&managed, &lease_id("source")).await?;
    let SnapshotExportOutcome::Native(receipt) = provider.snapshot_lease(&lease_id("source")).await else {
        return Err("no receipt".into());
    };

    fake.on(
        "POST",
        "/v3/sandboxes",
        200,
        json!({
            "sandbox": {"name": "sbx-b", "persistent": true},
            "session": {"id": "sess_b"},
        }),
    );
    let restore = request_with(|wire| {
        wire["spec"]["image"] = json!({
            "kind": "native_snapshot",
            "reference": {"provider": "vercel", "id": receipt.reference.id.as_str()}
        });
    })?;
    let restored = provider.lease(&restore, &lease_id("fork")).await?;
    assert_eq!(restored.sandbox_id.as_str(), "sbx-b");
    let creates = fake.calls("POST", "/v3/sandboxes");
    assert_eq!(
        creates.first().ok_or("no create")?.body["image"],
        "vercel/sandbox/node:24"
    );
    assert_eq!(
        creates.last().ok_or("no create")?.body["source"]["snapshotId"],
        "snap_9"
    );

    fake.on("DELETE", SANDBOX, 200, json!({}));
    fake.on("DELETE", "/v2/sandboxes/sbx-b", 200, json!({}));
    provider.cancel(&lease_id("source")).await?;
    assert_eq!(provider.active_lease_count().await, 1);
    provider.cancel(&lease_id("fork")).await?;
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_snapshot_that_waited_on_a_cancel_finds_the_lease_gone() -> TestResult {
    let (deleting, deleted) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let deleting = Mutex::new(deleting);
    let released = Mutex::new(released);
    let fake = Fake::start_with(move |request| {
        if request.method == "DELETE" {
            let _ = deleting.lock().map(|sender| sender.send(()));
            let _ = released
                .lock()
                .map(|receiver| receiver.recv_timeout(std::time::Duration::from_secs(10)));
        }
    })
    .await?;
    fake.created();
    fake.on("DELETE", SANDBOX, 200, json!({}));
    let provider = Arc::new(fake.provider()?);
    let id = lease_id("l1");
    provider.lease(&request()?, &id).await?;
    assert!(Arc::ptr_eq(provider.exec(), provider.exec()));

    let canceller = {
        let (provider, id) = (provider.clone(), id.clone());
        tokio::spawn(async move { provider.cancel(&id).await })
    };
    tokio::task::spawn_blocking(move || deleted.recv_timeout(std::time::Duration::from_secs(10))).await??;
    let snapper = {
        let (provider, id) = (provider.clone(), id.clone());
        tokio::spawn(async move { provider.snapshot_lease(&id).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    release.send(())?;
    within(canceller).await??.map_err(|error| error.to_string())?;
    assert!(matches!(
        within(snapper).await??,
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { .. })
    ));
    Ok(())
}
