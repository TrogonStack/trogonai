#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod native_recovery_response_support;
mod worker_process_support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt;
use serde_json::json;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, SandboxFreezeRequest,
    SandboxFreezeResponse, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects};

use ensure_support::{Boxed, Context, check};
use native_access_support::NativeAccess;
use native_command_support::{Binding, Guest, NativeEnvd};
use native_process_support::{Api, Control, SANDBOX_ID};
use native_recovery_response_support::{Facade, Fault};
use worker_process_support::{Nats, Process, signed};

const LAUNCH_KEY: &str = "native-recovery-owned-start";
const EFFECT: &str = "native-recovery-effect";

#[derive(Eq, PartialEq)]
struct BucketSnapshot {
    sequence: u64,
    values: BTreeMap<String, Vec<u8>>,
}

async fn snapshot(context: &jetstream::Context, bucket: &str) -> Result<BucketSnapshot, Boxed> {
    let store = context.get_key_value(bucket).await?;
    let mut stream = store.stream.clone();
    let sequence = stream.info().await?.state.last_sequence;
    let mut values = BTreeMap::new();
    let mut keys = store.keys().await?;
    while let Some(key) = keys.next().await {
        let key = key?;
        let value = store
            .get(&key)
            .await?
            .context("retained native authority disappeared during observation")?;
        values.insert(key, value.to_vec());
    }
    check(
        !values.is_empty() && stream.info().await?.state.last_sequence == sequence,
        "positive retained authority must be nonempty and stable",
    )?;
    Ok(BucketSnapshot { sequence, values })
}

fn counters(control: &Control) -> [usize; 8] {
    [
        &control.creates,
        &control.deletes,
        &control.connects,
        &control.lists,
        &control.gets,
        &control.foreign_gets,
        &control.foreign_deletes,
        &control.pauses,
    ]
    .map(|counter| counter.load(Ordering::SeqCst))
}

fn native_controls(guest: &Guest, sentinel: Binding, target: Binding) -> Result<(), Boxed> {
    check(
        !guest.group_members(sentinel)?.is_empty() && !guest.group_members(target)?.is_empty(),
        "native recovery refusal must preserve the exact target and sentinel groups",
    )?;
    check(
        guest.effect(EFFECT)?.as_deref() == Some(b"owned\n".as_slice()),
        "native recovery must retain one real original user effect",
    )?;
    check(
        guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
        "native reconstruction must not launch another bootstrap",
    )
}

/// The restarted worker must exit without ever advertising readiness.
async fn observe_refusal(worker: &mut Process, heartbeats: &mut async_nats::Subscriber) -> Result<(), Boxed> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while worker.is_alive()? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Boxed>(())
    })
    .await??;
    check(
        tokio::time::timeout(Duration::from_millis(100), heartbeats.next())
            .await
            .is_err(),
        "a refused recovery must never advertise readiness",
    )
}

async fn recovery_refusal(fault: Fault) -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = jetstream::new(nats.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let native = NativeAccess::start(envd.url.to_string(), Arc::clone(&api.control)).await?;
    let facade = Facade::start(&api.url).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("native-recovery-fixture", [71; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = worker_process_support::worker_with_epoch_mode(
        &nats.url,
        &facade.url,
        &native.url,
        &ledger,
        &signer,
        10,
        EpochMode::Require,
    )?;
    let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
        .await?
        .context("the healthy producer did not become ready through the facade")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    check(
        heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
        "actual producer must advertise observed epoch support and targeted dispatch",
    )?;
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("native-recovery-attempt")?,
        epoch: Some(LeaseEpoch::new(7)?),
    };
    let lease_id = LeaseId::new("native-recovery-lease");
    let mut request = authority_support::lease_request()?;
    request.ttl_seconds = 600;
    let scope = VerbScope {
        tenant: request.tenant.clone(),
        lease_id: lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    let dispatch = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let grant: ProviderLease = worker_process_support::dispatch(&nats, &signer, &dispatch, &target.worker_id).await?;
    check(
        grant.sandbox_id.as_str() == SANDBOX_ID
            && grant.worker_id.as_ref() == Some(&target.worker_id)
            && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == target.epoch,
        "the actual positive grant must bind the exact owned native assignment",
    )?;
    let effect_path = guest.path(EFFECT).display().to_string();
    let observed = async {
        let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
        let sentinel: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"command":["sleep","300"],"detached":true,"deadline":"lease","run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        check(matches!(response, RunCommandResponse::Started(_)), "the ordinary native sentinel must be a healthy positive control")?;
        let sentinel = guest.last_binding()?;
        let positive: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"launch_key":LAUNCH_KEY,"require_launch_cancellation":true,
            "command":["bash","-c",format!("echo owned >>{effect_path}; sleep 300 & wait")],
            "detached":true,"timeout_seconds":180,"run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err("the strong native positive control must establish durable custody".into());
        };
        let execution_id = started.execution_id;
        let native_target = guest.last_binding()?;
        check(native_target.pid != sentinel.pid, "native target and sentinel must have independent exact identities")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while guest.effect(EFFECT)?.as_deref() != Some(b"owned\n".as_slice()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, Boxed>(())
        })
        .await??;
        native_controls(&guest, sentinel, native_target)?;
        let before_assignments = snapshot(&context, "sandbox-e2b-assignments").await?;
        let before_commands = snapshot(&context, "sandbox-e2b-command-custody").await?;
        let producer_pid = worker.id();
        worker.kill()?;
        let original_fence = std::fs::read(ledger.with_extension("fence"))?;
        let before_native = counters(&api.control);
        let before_facade = facade.counts();
        api.control.recovered_access.store(true, Ordering::SeqCst);
        facade.set_fault(Some(fault));
        drop(heartbeats);
        heartbeats = nats.client.subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b)).await?;
        nats.client.flush().await?;
        worker = worker_process_support::worker_with_epoch_mode(&nats.url, &facade.url, &native.url, &ledger, &signer, 10, EpochMode::Require)?;
        let failed_pid = worker.id();
        observe_refusal(&mut worker, &mut heartbeats).await?;
        let after_native = counters(&api.control);
        let after_facade = facade.counts();
        let assignments_unchanged = snapshot(&context, "sandbox-e2b-assignments").await? == before_assignments;
        let commands_unchanged = snapshot(&context, "sandbox-e2b-command-custody").await? == before_commands;
        let fence_unchanged = std::fs::read(ledger.with_extension("fence"))? == original_fence;
        native_controls(&guest, sentinel, native_target)?;
        worker.kill()?;
        facade.set_fault(None);
        drop(heartbeats);
        heartbeats = nats.client.subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b)).await?;
        nats.client.flush().await?;
        worker = worker_process_support::worker_with_epoch_mode(&nats.url, &facade.url, &native.url, &ledger, &signer, 10, EpochMode::Require)?;
        let restored = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
            .await?
            .context("restoring the exact native response did not restore native custody")?;
        let restored: WorkerHeartbeat = serde_json::from_slice(&restored.payload)?;
        check(
            worker.is_alive()?
                && worker.id() != producer_pid
                && worker.id() != failed_pid
                && restored.worker_id == target.worker_id
                && restored.capacity.current_leases == 1
                && restored.supports_lease_epoch.is_supported()
                && restored.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
            "normal exact GET must restore a distinct actual Require process with occupied native ownership",
        )?;
        let replay: RunCommandResponse = signed(&nats, &signer, run_subject, &positive, &scope).await?;
        check(
            matches!(replay, RunCommandResponse::Started(ref started) if started.execution_id == execution_id)
                && snapshot(&context, "sandbox-e2b-assignments").await? == before_assignments
                && snapshot(&context, "sandbox-e2b-command-custody").await? == before_commands
                && std::fs::read(ledger.with_extension("fence"))? == original_fence,
            "response restoration alone must recover the original execution and exact authority without rewriting stores or fence",
        )?;
        native_controls(&guest, sentinel, native_target)?;
        let request = CancelCommandLaunchRequest { lease_id: lease_id.clone(), launch_key: LaunchKey::new(LAUNCH_KEY)? };
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
        let canceled: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &request, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject, &request, &scope).await?;
        check(
            canceled == CancelCommandLaunchResponse::Canceled { execution_id: Some(execution_id) }
                && canceled == replay
                && guest.group_members(native_target)?.is_empty()
                && !guest.group_members(sentinel)?.is_empty()
                && api.control.deletes.load(Ordering::SeqCst) == 0,
            "restored exact key cancellation must settle and replay only its original group, preserving the native sentinel",
        )?;
        api.control.allow_lifecycle.store(true, Ordering::SeqCst);
        let request = SandboxFreezeRequest { lease_id: lease_id.clone() };
        let paused: SandboxFreezeResponse =
            signed(&nats, &signer, subjects::provider_pause(ProviderKind::E2b, &lease_id), &request, &scope).await?;
        check(matches!(paused, SandboxFreezeResponse::Done), "the restored native lifetime control must pause")?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let resumed: SandboxFreezeResponse =
            signed(&nats, &signer, subjects::provider_resume(ProviderKind::E2b, &lease_id), &request, &scope).await?;
        let timeouts = api.control.resume_timeouts.lock().unwrap_or_else(PoisonError::into_inner).clone();
        check(
            matches!(resumed, SandboxFreezeResponse::Done)
                && timeouts.len() == 1
                && timeouts.iter().all(|(requested, maximum)| *requested > 0 && requested <= maximum && *requested < 600),
            "response restoration must preserve the original absolute native lifetime rather than reset requested TTL",
        )?;
        check(
            native.recovered.load(Ordering::SeqCst) > 0
                && native.rejected.load(Ordering::SeqCst) == 0
                && !guest.group_members(sentinel)?.is_empty()
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
            "the restored control must use fresh native credentials and preserve the exact sentinel without new effects",
        )?;
        Ok::<_, Boxed>((
            before_native,
            after_native,
            before_facade,
            after_facade,
            assignments_unchanged,
            commands_unchanged,
            fence_unchanged,
        ))
    }
    .await;

    facade.set_fault(None);
    let cleanup_request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id,
            provider: ProviderKind::E2b,
            target,
        },
        intent: CancelIntent::Release,
    };
    let cleanup: Result<ProviderCleanupResponse, Boxed> = signed(
        &nats,
        &signer,
        subjects::worker_cleanup(ProviderKind::E2b, &cleanup_request.identity.target.worker_id),
        &cleanup_request,
        &scope,
    )
    .await;
    let product_removed = api.control.product_removed.load(Ordering::SeqCst);
    let final_counters = counters(&api.control);
    worker.stop()?;
    facade.shutdown().await;
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    let (
        before_native,
        after_native,
        before_facade,
        after_facade,
        assignments_unchanged,
        commands_unchanged,
        fence_unchanged,
    ) = observed?;
    check(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. }))
            && product_removed
            && final_counters[0] == 1
            && final_counters[1] == 1
            && final_counters[2] == 1
            && final_counters[3] == 0
            && final_counters[4] >= 3
            && final_counters[5] == 0
            && final_counters[6] == 0
            && final_counters[7] == 1,
        "restored exact authority must end in targeted physical native cleanup without invented deletion authority",
    )?;
    let mut expected_native = before_native;
    expected_native[4] += 1;
    check(
        after_native == expected_native
            && after_facade.exact_gets == before_facade.exact_gets + 1
            && after_facade.upstream_successes == before_facade.upstream_successes + 1
            && after_facade.injected == before_facade.injected + 1
            && assignments_unchanged
            && commands_unchanged
            && fence_unchanged,
        &format!(
            "foreign, uncertain or unadoptable exact native GET must refuse adoption and preserve authority: fault={fault:?}"
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_native_identity_refuses_recovery() -> Result<(), Boxed> {
    recovery_refusal(Fault::ForeignIdentity).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_not_found_preserves_recovery_authority() -> Result<(), Boxed> {
    recovery_refusal(Fault::NotFound).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_unauthorized_refuses_recovery() -> Result<(), Boxed> {
    recovery_refusal(Fault::Unauthorized).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_unavailable_refuses_recovery() -> Result<(), Boxed> {
    recovery_refusal(Fault::Unavailable).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paused_native_state_refuses_automatic_adoption() -> Result<(), Boxed> {
    recovery_refusal(Fault::Paused).await
}
