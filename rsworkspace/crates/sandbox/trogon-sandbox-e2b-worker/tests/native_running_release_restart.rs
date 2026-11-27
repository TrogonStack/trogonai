#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod native_running_release_support;
mod restart_support;
mod worker_process_support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use serde_json::Value;
use trogon_sandbox_contract::{CancelCommandLaunchResponse, LeaseError, RunCommandResponse, WorkerHeartbeat};
use trogon_sandbox_protocol::AUTH_HEADER;

use ensure_support::{Boxed, Context};
use native_access_support::NativeAccess;
use native_process_support::SANDBOX_ID;
use native_running_release_support::HeldRelease;
use restart_support::{Fixture, Lease, assert_targeted};

const LAUNCH_KEY: &str = "running-release-original";
const POSITIVE_EFFECT: &str = "running-release-positive";
const TARGET_EFFECT: &str = "running-release-target";

/// Publishes a signed request whose reply nobody reads, and returns the inbox it would have gone to.
async fn outstanding<T: serde::Serialize>(
    fixture: &Fixture,
    lease: &Lease,
    subject: String,
    request: &T,
) -> Result<async_nats::Subscriber, Boxed> {
    let inbox = fixture.nats.client.new_inbox();
    let subscriber = fixture.nats.client.subscribe(inbox.clone()).await?;
    fixture.nats.client.flush().await?;
    let payload = serde_json::to_vec_pretty(request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, fixture.signer.sign_verb(&subject, &payload, &lease.scope)?);
    fixture
        .nats
        .client
        .publish_with_reply_and_headers(subject, inbox, headers, payload.into())
        .await?;
    fixture.nats.client.flush().await?;
    Ok(subscriber)
}

/// The custody record under `key`, once it satisfies `ready`.
async fn custody_when(fixture: &Fixture, key: &str, ready: impl Fn(&Value) -> bool) -> Result<Value, Boxed> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(store) = fixture.context.get_key_value("sandbox-e2b-command-custody").await
                && let Some(bytes) = store.get(key).await?
            {
                let record: Value = serde_json::from_slice(&bytes)?;
                if ready(&record) {
                    return Ok::<_, Boxed>(record);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("the command custody record did not reach the expected state")?
}

/// Whether `after` keeps every immutable field of `before`.
fn immutable(before: &Value, after: &Value) -> bool {
    [
        "version",
        "owner",
        "launch_key",
        "execution_id",
        "fingerprint",
        "execution",
    ]
    .into_iter()
    .all(|field| after[field] == before[field])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_running_cancel_survives_worker_loss_and_rejects_the_old_authenticated_release() -> Result<(), Boxed> {
    let fixture = Fixture::start("running-release-fixture", 73).await?;
    let mut held = HeldRelease::start(&fixture.envd.url).await?;
    let held_native = NativeAccess::start(held.url.clone(), std::sync::Arc::clone(&fixture.api.control)).await?;
    let (mut worker, heartbeat) = fixture.boot(&fixture.api.url, &held_native.url).await?;
    assert_targeted(&heartbeat);
    let producer_pid = worker.id();
    let lease = Lease::new(&heartbeat, "running-release", &[])?;
    lease.grant(&fixture).await?;

    let positive = lease.strong(&fixture, "running-release-positive", POSITIVE_EFFECT)?;
    let (positive_id, positive_binding) = lease.start_strong(&fixture, &positive, POSITIVE_EFFECT).await?;
    let canceled = lease.cancel_launch(&fixture, "running-release-positive").await?;
    let replay = lease.cancel_launch(&fixture, "running-release-positive").await?;
    assert!(
        canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(positive_id)
            }
            && replay == canceled
            && fixture.guest.group_members(positive_binding)?.is_empty(),
        "intact strong cancellation and replay must remove the positively running native group"
    );
    let sentinel = lease.sentinel(&fixture).await?;

    let command_key = lease.command_key(LAUNCH_KEY)?;
    let assignment_key = lease.assignment_key()?;
    let original = lease.strong(&fixture, LAUNCH_KEY, TARGET_EFFECT)?;
    held.arm();
    let mut old_run = outstanding(&fixture, &lease, lease.run_subject(), &original).await?;
    let binding = held.reached().await?;
    let before = custody_when(&fixture, &command_key, |record| record["phase"] == "running").await?;
    let execution_id = before["execution_id"]
        .as_str()
        .context("missing original host execution identity")?
        .to_owned();
    assert!(
        before["cancel_requested"] == false
            && before["execution"]["identity"]["pid"].as_u64() == Some(u64::from(binding.pid))
            && before["execution"]["identity"]["started"].as_u64() == Some(binding.started)
            && before["owner"]["sandbox_id"] == SANDBOX_ID
            && before["owner"]["worker_id"] == lease.target.worker_id.as_str()
            && before["owner"]["attempt_id"] == lease.target.attempt_id.as_str()
            && before["owner"]["epoch"] == restart_support::EPOCH
            && before["owner"]["lease_id"] == lease.id.as_str(),
        "the held authenticated Release must bind the exact original durable Running identity and authority"
    );
    let captured = fixture.guest.last_binding()?;
    assert!(
        captured.pid == binding.pid
            && captured.started == binding.started
            && binding.pid != sentinel.pid
            && !fixture.guest.group_members(binding)?.is_empty()
            && held.pending()
            && fixture.guest.effect(TARGET_EFFECT)?.is_none()
            && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 3,
        "the held original must be genuinely alive with no target user effect"
    );
    let assignments = fixture.context.get_key_value("sandbox-e2b-assignments").await?;
    let assignment = assignments
        .get(&assignment_key)
        .await?
        .context("missing exact durable native assignment")?;
    let fence_path = fixture.ledger.with_extension("fence");
    let fence = std::fs::read(&fence_path)?;
    assert!(!fence.is_empty(), "the worker must retain its durable fence");

    let cancel = trogon_sandbox_contract::CancelCommandLaunchRequest {
        lease_id: lease.id.clone(),
        launch_key: trogon_sandbox_contract::LaunchKey::new(LAUNCH_KEY)?,
    };
    let mut old_cancel = outstanding(&fixture, &lease, lease.cancel_subject(), &cancel).await?;
    let intent = custody_when(&fixture, &command_key, |record| record["cancel_requested"] == true).await?;
    assert!(
        intent["phase"] == "running"
            && immutable(&before, &intent)
            && held.pending()
            && old_run.next().now_or_never().is_none()
            && old_cancel.next().now_or_never().is_none()
            && !fixture.guest.group_members(binding)?.is_empty()
            && worker.is_alive()?,
        "durable cancellation must precede worker loss, while both replies remain outstanding"
    );
    worker.stop()?;
    drop((old_run, old_cancel));
    fixture.api.control.recovered_access.store(true, Ordering::SeqCst);
    let (mut worker, recovered_heartbeat): (_, WorkerHeartbeat) =
        fixture.boot(&fixture.api.url, &held_native.url).await?;
    assert!(
        worker.is_alive()?
            && producer_pid != worker.id()
            && recovered_heartbeat.worker_id == lease.target.worker_id
            && recovered_heartbeat.capacity.current_leases == 1,
        "the new worker must recover the same occupied assignment"
    );
    assert!(
        !fixture.guest.group_members(binding)?.is_empty()
            && held.pending()
            && fixture.guest.effect(TARGET_EFFECT)?.is_none(),
        "the original held group must still be alive before recovered cancellation"
    );
    let replayed: RunCommandResponse = lease.request(&fixture, lease.run_subject(), &original).await?;
    assert!(
        matches!(&replayed, RunCommandResponse::Denied(LeaseError::InvalidSpec { reason })
            if reason == "the command launch was canceled"),
        "the canceled launch must refuse its own replay: {replayed:?}"
    );
    let canceled = lease.cancel_launch(&fixture, LAUNCH_KEY).await?;
    let replay = lease.cancel_launch(&fixture, LAUNCH_KEY).await?;
    let settled = CancelCommandLaunchResponse::Canceled {
        execution_id: Some(trogon_sandbox_contract::ExecutionId::new(execution_id)),
    };
    assert!(
        canceled == settled && replay == settled && fixture.guest.group_members(binding)?.is_empty(),
        "the recovered worker must cancel the original native group exactly: {canceled:?}"
    );
    let after = custody_when(&fixture, &command_key, |record| record["phase"] == "canceled").await?;
    assert!(
        immutable(&before, &after)
            && after["cancel_requested"] == true
            && assignments
                .get(&assignment_key)
                .await?
                .is_some_and(|bytes| bytes == assignment)
            && std::fs::read(&fence_path)? == fence,
        "cancellation must leave the original authority untouched"
    );

    // Whether the native script exits nonzero depends on whether the host reaps the canceled process, so only
    // its completion is asserted; the absence of any user or bootstrap effect is what proves the release inert.
    let late = held.forward_late().await?;
    let final_record = custody_when(&fixture, &command_key, |_| true).await?;
    assert!(
        late.forwarded
            && late.native_exit.is_some()
            && final_record == after
            && fixture.guest.group_members(binding)?.is_empty()
            && !fixture.guest.group_members(sentinel)?.is_empty()
            && fixture.guest.effect(TARGET_EFFECT)?.is_none()
            && fixture.guest.effect(POSITIVE_EFFECT)?.as_deref() == Some(b"owned\n".as_slice())
            && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 3,
        "the already authenticated original Release must be inert natively without any user or bootstrap effect: {:?}",
        late.native_exit
    );
    let control = &fixture.api.control;
    assert!(
        control.gets.load(Ordering::SeqCst) > 0
            && held_native.recovered.load(Ordering::SeqCst) > 0
            && held_native.rejected.load(Ordering::SeqCst) == 0
            && control.creates.load(Ordering::SeqCst) == 1
            && control.connects.load(Ordering::SeqCst) == 0
            && control.lists.load(Ordering::SeqCst) == 0
            && control.pauses.load(Ordering::SeqCst) == 0
            && control.deletes.load(Ordering::SeqCst) == 0
            && control.foreign_gets.load(Ordering::SeqCst) == 0
            && control.foreign_deletes.load(Ordering::SeqCst) == 0,
        "recovery must use fresh exact read-only native access"
    );
    let released = lease.release(&fixture).await?;
    worker.stop()?;
    held.shutdown().await?;
    held_native.shutdown().await;
    assert!(
        matches!(
            released,
            trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { .. }
        ) && fixture.api.control.product_removed.load(Ordering::SeqCst),
        "the final targeted cleanup must establish native absence: {released:?}"
    );
    fixture.finish().await
}
