#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod worker_process_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    ExecutionId, LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, SandboxFreezeRequest,
    SandboxFreezeResponse, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects, tenant_scoped_kv_key};

use ensure_support::{Boxed, Context};
use native_access_support::NativeAccess;
use native_command_support::{Guest, NativeEnvd};
use native_process_support::{Api, SANDBOX_ID};
use worker_process_support::{Nats, signed};

const EFFECTS: &str = "production-restart-count";

/// Waits until the command under test has run once, which only becomes visible after its custody is Running.
async fn first_effect(guest: &Guest) -> Result<(), Boxed> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while guest.effect(EFFECTS)?.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Boxed>(())
    })
    .await
    .context("the owned native effect did not become observable")?
}

/// How many times the command under test ran, read from the file it appends to.
fn effects(guest: &Guest) -> Result<usize, Boxed> {
    let bytes = guest
        .effect(EFFECTS)?
        .context("the owned native effect counter is unavailable")?;
    let text = std::str::from_utf8(&bytes)?;
    if !text.lines().all(|line| line == "launched") {
        return Err("the owned native effect counter has invalid data".into());
    }
    Ok(text.lines().count())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_start_reply_retains_native_authority_and_capacity_after_actual_worker_restart() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = async_nats::jetstream::new(nats.client.clone());
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let native = NativeAccess::start(envd.url.clone(), Arc::clone(&api.control)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("production-restart-fixture", [43; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = worker_process_support::worker_with_epoch_mode(
        &nats.url,
        &api.url,
        &native.url,
        &ledger,
        &signer,
        10,
        EpochMode::Require,
    )?;
    let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
        .await?
        .context("the producer worker did not become ready")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    let producer_pid = worker.id();
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("production-restart-attempt")?,
        epoch: Some(LeaseEpoch::FIRST),
    };
    let lease_id = LeaseId::new("production-restart-lease");
    let mut request = authority_support::lease_request()?;
    request.ttl_seconds = 600;
    let tenant = request.tenant.clone();
    let scope = VerbScope {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: Some(LeaseEpoch::FIRST),
    };
    let dispatch = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let grant: ProviderLease = worker_process_support::dispatch(&nats, &signer, &dispatch, &target.worker_id).await?;
    assert!(
        grant.sandbox_id.as_str() == SANDBOX_ID
            && grant.worker_id.as_ref() == Some(&target.worker_id)
            && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == target.epoch,
        "producer grant must bind the exact native assignment"
    );
    let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
    let observed = async {
        let sentinel: RunCommandRequest = serde_json::from_value(json!({ "lease_id":lease_id,"command":["sleep","60"],"detached":true,"deadline":"lease","run_as_user":"root" }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        assert!(matches!(response, RunCommandResponse::Started(_)), "the production native sentinel positive control must start: {response:?}");
        let sentinel = guest.last_binding()?;
        let script = format!("echo launched >>{}; sleep 60 & wait", guest.path(EFFECTS).display());
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"launch_key":"restart-owned-start","require_launch_cancellation":true,
            "command":["bash","-c",script],
            "detached":true,"timeout_seconds":60,"run_as_user":"root",
        }))?;
        let payload = serde_json::to_vec_pretty(&request)?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(trogon_sandbox_protocol::AUTH_HEADER, signer.sign_verb(&run_subject, &payload, &scope)?);
        let lost_reply = nats.client.new_inbox();
        nats.client.publish_with_reply_and_headers(run_subject.clone(), lost_reply, headers, payload.into()).await?;
        nats.client.flush().await?;
        let key = tenant_scoped_kv_key(&tenant, &serde_json::to_string(&(lease_id.as_str(), "restart-owned-start"))?);
        let (store, before) = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Ok(store) = context.get_key_value("sandbox-e2b-command-custody").await
                    && let Some(bytes) = store.get(&key).await?
                {
                    let record: Value = serde_json::from_slice(&bytes)?;
                    if record["phase"] == "running" && guest.bootstrap_calls.load(Ordering::SeqCst) == 2 {
                        return Ok::<_, Boxed>((store, record));
                    }
                }
                if !worker.is_alive()? {
                    return Err("the producer process exited before native start custody".into());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.context("the actual strong producer did not establish Running custody with its reply suppressed")??;
        first_effect(&guest).await?;
        let native_target = guest.last_binding()?;
        assert!(native_target.pid != sentinel.pid && !guest.group_members(native_target)?.is_empty() && effects(&guest)? == 1, "the blind producer must establish exactly one real user effect and native group");
        let original_execution_id = before["execution_id"].as_str().context("missing retained host execution identity")?.to_owned();
        let original_deadline = before["execution"]["deadline"].clone();
        assert!(!original_deadline.is_null(), "the producer must retain the original native deadline");
        worker.kill()?;
        drop(heartbeats);
        api.control.recovered_access.store(true, Ordering::SeqCst);
        let mut restarted_heartbeats = nats.client.subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b)).await?;
        nats.client.flush().await?;
        worker = worker_process_support::worker_with_epoch_mode(&nats.url, &api.url, &native.url, &ledger, &signer, 10, EpochMode::Require)?;
        let restarted_pid = worker.id();
        let recovered = tokio::time::timeout(Duration::from_secs(15), restarted_heartbeats.next()).await?.context("the restarted production process did not become ready")?;
        let recovered: WorkerHeartbeat = serde_json::from_slice(&recovered.payload)?;
        assert!(worker.is_alive()? && producer_pid != restarted_pid, "recovery must run in a distinct actual worker process");
        let request = CancelCommandLaunchRequest { lease_id: lease_id.clone(), launch_key: LaunchKey::new("restart-owned-start")? };
        let canceled: CancelCommandLaunchResponse = signed(&nats, &signer, subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id), &request, &scope).await?;
        let exact_gets = api.control.gets.load(Ordering::SeqCst);
        let fresh_native_calls = native.recovered.load(Ordering::SeqCst);
        let rejected_native_calls = native.rejected.load(Ordering::SeqCst);
        let no_recovery_mutation = api.control.connects.load(Ordering::SeqCst) == 0
            && api.control.pauses.load(Ordering::SeqCst) == 0
            && api.control.deletes.load(Ordering::SeqCst) == 0
            && api.control.lists.load(Ordering::SeqCst) == 0;
        let remaining = guest.group_members(native_target)?;
        let unrelated = guest.group_members(sentinel)?;
        let user_effects = effects(&guest)?;
        let after: Value = serde_json::from_slice(&store.get(&key).await?.context("retained command custody disappeared")?)?;
        api.control.allow_lifecycle.store(true, Ordering::SeqCst);
        let request = SandboxFreezeRequest { lease_id: lease_id.clone() };
        let paused: SandboxFreezeResponse = signed(&nats, &signer, subjects::provider_pause(ProviderKind::E2b, &lease_id), &request, &scope).await?;
        assert!(matches!(paused, SandboxFreezeResponse::Done), "the explicitly authorized pause must complete");
        tokio::time::sleep(Duration::from_millis(3200)).await;
        let resumed: SandboxFreezeResponse = signed(&nats, &signer, subjects::provider_resume(ProviderKind::E2b, &lease_id), &request, &scope).await?;
        let resume_timeouts = api.control.resume_timeouts.lock().map_err(|_| "the resume record is poisoned")?.clone();
        let bootstraps = guest.bootstrap_calls.load(Ordering::SeqCst);
        let creates = api.control.creates.load(Ordering::SeqCst);
        Ok::<_, Boxed>((recovered, canceled, original_execution_id, original_deadline, after, exact_gets, fresh_native_calls, rejected_native_calls, no_recovery_mutation, remaining, unrelated, user_effects, resumed, resume_timeouts, bootstraps, creates))
    }.await;
    let cleanup_request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease_id.clone(),
            provider: ProviderKind::E2b,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let cleanup: Result<ProviderCleanupResponse, Boxed> = signed(
        &nats,
        &signer,
        subjects::worker_cleanup(ProviderKind::E2b, &target.worker_id),
        &cleanup_request,
        &scope,
    )
    .await;
    let product_removed = api.control.product_removed.load(Ordering::SeqCst);
    let foreign_gets = api.control.foreign_gets.load(Ordering::SeqCst);
    let foreign_deletes = api.control.foreign_deletes.load(Ordering::SeqCst);
    worker.stop()?;
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    let (
        recovered,
        canceled,
        original_execution_id,
        original_deadline,
        after,
        exact_gets,
        fresh_native_calls,
        rejected_native_calls,
        no_recovery_mutation,
        remaining,
        unrelated,
        user_effects,
        resumed,
        resume_timeouts,
        bootstraps,
        creates,
    ) = observed
        .with_context(|| format!("owned worker processes stopped=true; product_native_cleanup={product_removed}"))?;
    assert!(
        recovered.worker_id == target.worker_id && recovered.capacity.current_leases == 1 && exact_gets > 0,
        "the first ready heartbeat after actual process restart must retain native authority and occupied capacity: occupied={}, exact_native_gets={exact_gets}, cancel={canceled:?}",
        recovered.capacity.current_leases
    );
    assert!(
        canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(ExecutionId::new(original_execution_id))
            }
            && remaining.is_empty()
            && !unrelated.is_empty(),
        "a permanently unobserved start reply must remain cancelable after actual process loss without harming the native sentinel: {canceled:?}"
    );
    assert!(
        fresh_native_calls > 0
            && rejected_native_calls == 0
            && no_recovery_mutation
            && foreign_gets == 0
            && foreign_deletes == 0,
        "recovery must use fresh exact read-only native access, without connect/pause/delete/list or foreign identity fallback"
    );
    assert!(
        after["execution"]["deadline"] == original_deadline && bootstraps == 2 && creates == 1 && user_effects == 1,
        "process restart must preserve the original deadline and avoid duplicate user/native effects"
    );
    assert!(
        matches!(resumed, SandboxFreezeResponse::Done)
            && resume_timeouts.len() == 1
            && resume_timeouts[0].0 > 0
            && resume_timeouts[0].0 <= resume_timeouts[0].1
            && resume_timeouts[0].0 < 600,
        "an explicitly authorized resume after recovery must retain the authoritative absolute native lifetime: observed_timeouts={resume_timeouts:?}"
    );
    assert!(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. })) && product_removed,
        "explicit final targeted cleanup must confirm physical native absence: cleanup={cleanup:?}"
    );
    Ok(())
}
