#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod worker_process_support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream::{self, stream};
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse,
    SandboxId, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects};

use ensure_support::{Boxed, Context, check};
use native_access_support::NativeAccess;
use native_command_support::{Binding, Guest, NativeEnvd};
use native_process_support::{Api, Control, SANDBOX_ID};
use worker_process_support::{Nats, signed};

const ASSIGNMENTS: &str = "sandbox-e2b-assignments";
const COMMANDS: &str = "sandbox-e2b-command-custody";
const LAUNCH_KEY: &str = "assignment-policy-owned-start";
const EFFECT: &str = "assignment-policy-effect";
const FORBIDDEN: &str = "assignment-policy-forbidden";

#[derive(Clone, Copy, Debug)]
enum Policy {
    Expiring,
    Evicting,
    Full,
}

#[derive(Eq, PartialEq)]
struct BucketSnapshot {
    sequence: u64,
    values: BTreeMap<String, Vec<u8>>,
    config: Value,
}

async fn snapshot(context: &jetstream::Context, bucket: &str) -> Result<BucketSnapshot, Boxed> {
    let store = context.get_key_value(bucket).await?;
    let mut native_stream = store.stream.clone();
    let info = native_stream.info().await?;
    let sequence = info.state.last_sequence;
    let config = serde_json::to_value(&info.config)?;
    let mut values = BTreeMap::new();
    let mut keys = store.keys().await?;
    while let Some(key) = keys.next().await {
        let key = key?;
        let value = store
            .get(&key)
            .await?
            .context("owned storage authority disappeared during observation")?;
        values.insert(key, value.to_vec());
    }
    check(
        !values.is_empty() && native_stream.info().await?.state.last_sequence == sequence,
        "owned native authority must be stable and nonempty",
    )?;
    Ok(BucketSnapshot {
        sequence,
        values,
        config,
    })
}

async fn restore_policy(context: &jetstream::Context, original: &stream::Config) -> Result<(), Boxed> {
    context.update_stream(original.clone()).await?;
    let mut restored = context.get_stream(&original.name).await?;
    check(
        serde_json::to_value(&restored.info().await?.config)? == serde_json::to_value(original)?,
        "NATS must restore the exact original assignment policy",
    )
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
        "assignment policy refusal must preserve exact target and sentinel groups",
    )?;
    check(
        guest.effect(EFFECT)?.as_deref() == Some(b"owned\n".as_slice()),
        "the original user effect must remain unique",
    )?;
    check(
        guest.effect(FORBIDDEN)?.is_none() && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
        "refusal must positively preserve forbidden-effect absence and the native bootstrap budget",
    )
}

async fn policy_refusal(policy: Policy) -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = jetstream::new(nats.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let native = NativeAccess::start(envd.url.to_string(), Arc::clone(&api.control)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("assignment-policy-fixture", [73; 32]);
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
        .context("the healthy production worker did not become ready")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    check(
        heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
        "the actual producer must advertise epoch support and targeted dispatch",
    )?;
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("assignment-policy-attempt")?,
        epoch: Some(LeaseEpoch::new(7)?),
    };
    let lease_id = LeaseId::new("assignment-policy-lease");
    let mut request = authority_support::lease_request()?;
    request.ttl_seconds = 600;
    let scope = VerbScope {
        tenant: request.tenant.clone(),
        lease_id: lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    let positive_dispatch = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let reply: ProviderDispatchReply =
        worker_process_support::dispatch(&nats, &signer, &positive_dispatch, &target.worker_id).await?;
    let ProviderDispatchReply::Granted(grant) = reply else {
        return Err("the actual healthy targeted native grant must succeed".into());
    };
    let expected_grant = ProviderLease {
        sandbox_id: SandboxId::new(SANDBOX_ID)?,
        evidence_subject: subjects::evidence_subject(&lease_id),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    check(
        grant == expected_grant,
        "the healthy actual grant must retain the exact native assignment",
    )?;
    let store = context.get_key_value(ASSIGNMENTS).await?;
    let mut native_stream = store.stream.clone();
    let original_policy = native_stream.info().await?.config.clone();
    check(
        original_policy.max_age.is_zero()
            && original_policy.discard == stream::DiscardPolicy::New
            && original_policy.storage == stream::StorageType::File
            && !original_policy.allow_message_ttl,
        "the positive producer must first establish healthy durable assignment policy",
    )?;
    let effect_path = guest.path(EFFECT).display().to_string();
    let forbidden_path = guest.path(FORBIDDEN).display().to_string();
    let observed = async {
        let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
        let sentinel: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"command":["sleep","300"],"detached":true,"deadline":"lease","run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        check(
            matches!(response, RunCommandResponse::Started(_)),
            "the ordinary exact native sentinel must start",
        )?;
        let sentinel = guest.last_binding()?;
        let positive: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"launch_key":LAUNCH_KEY,"require_launch_cancellation":true,
            "command":["bash","-c",format!("echo owned >>{effect_path}; sleep 300 & wait")],
            "detached":true,"timeout_seconds":180,"run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err("the strong positive must establish real native custody before policy mutation".into());
        };
        let execution_id = started.execution_id;
        let native_target = guest.last_binding()?;
        check(
            native_target.pid != sentinel.pid,
            "positive native groups must have independent exact identities",
        )?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while guest.effect(EFFECT)?.as_deref() != Some(b"owned\n".as_slice()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, Boxed>(())
        })
        .await??;
        native_controls(&guest, sentinel, native_target)?;
        let healthy_assignments = snapshot(&context, ASSIGNMENTS).await?;
        let healthy_commands = snapshot(&context, COMMANDS).await?;
        let info = native_stream.info().await?;
        let mut changed = original_policy.clone();
        match policy {
            Policy::Expiring => changed.max_age = Duration::from_secs(3600),
            Policy::Evicting => {
                changed.discard = stream::DiscardPolicy::Old;
                changed.discard_new_per_subject = false;
            }
            Policy::Full => changed.max_messages = i64::try_from(info.state.messages)?,
        }
        context.update_stream(changed.clone()).await?;
        let mut accepted = context.get_stream(&changed.name).await?;
        check(
            serde_json::to_value(&accepted.info().await?.config)? == serde_json::to_value(&changed)?,
            "NATS must actually accept the precise intended policy mutation",
        )?;
        let before_assignments = snapshot(&context, ASSIGNMENTS).await?;
        let before_commands = snapshot(&context, COMMANDS).await?;
        check(
            before_assignments.values == healthy_assignments.values
                && before_assignments.sequence == healthy_assignments.sequence
                && before_commands == healthy_commands,
            "the accepted policy change itself must preserve existing native authority",
        )?;
        let before_native = counters(&api.control);
        let cancellation = CancelCommandLaunchRequest {
            lease_id: lease_id.clone(),
            launch_key: LaunchKey::new(LAUNCH_KEY)?,
        };
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
        if matches!(policy, Policy::Full) {
            let replay: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
            check(
                matches!(replay, RunCommandResponse::Started(ref started) if started.execution_id == execution_id),
                "full DiscardNew capacity must retain the existing valid execution replay",
            )?;
        } else {
            let forbidden: RunCommandRequest = serde_json::from_value(json!({
                "lease_id":lease_id,"launch_key":"assignment-policy-forbidden","require_launch_cancellation":true,
                "command":["bash","-c",format!("touch {forbidden_path}; sleep 300 & wait")],
                "detached":true,"timeout_seconds":180,"run_as_user":"root",
            }))?;
            let refused: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &forbidden, &scope).await?;
            check(
                matches!(refused, RunCommandResponse::Denied(_)),
                "unsafe cached assignment policy must deny new command effects on the held lease",
            )?;
            let refused: CancelCommandLaunchResponse =
                signed(&nats, &signer, cancel_subject.clone(), &cancellation, &scope).await?;
            check(
                matches!(refused, CancelCommandLaunchResponse::Denied(_)),
                "unsafe cached assignment policy must deny destructive command control",
            )?;
        }
        let refused_target = DispatchTarget {
            worker_id: target.worker_id.clone(),
            attempt_id: DispatchAttemptId::new("assignment-policy-refused-attempt")?,
            epoch: Some(LeaseEpoch::FIRST),
        };
        let refused_dispatch = ProviderDispatchPayload {
            lease_id: LeaseId::new("assignment-policy-refused-lease"),
            request: positive_dispatch.request.clone(),
            correlation_id: None,
            resolved_drives: vec![],
            target: Some(refused_target),
        };
        let refused: ProviderDispatchReply =
            worker_process_support::dispatch(&nats, &signer, &refused_dispatch, &target.worker_id).await?;
        check(
            matches!(refused, ProviderDispatchReply::LaunchFailed { ref provider_error } if !provider_error.is_empty()),
            "unsafe or full assignment storage must return an explicit reservation refusal before native allocation",
        )?;
        let after_assignments = snapshot(&context, ASSIGNMENTS).await?;
        let after_commands = snapshot(&context, COMMANDS).await?;
        let after_native = counters(&api.control);
        native_controls(&guest, sentinel, native_target)?;
        let assignment_unchanged = before_assignments == after_assignments;
        let command_unchanged = before_commands == after_commands;
        restore_policy(&context, &original_policy).await?;
        check(
            snapshot(&context, ASSIGNMENTS).await? == healthy_assignments
                && snapshot(&context, COMMANDS).await? == healthy_commands,
            "exact policy restoration must preserve the original authority values and sequences",
        )?;
        let replay: RunCommandResponse = signed(&nats, &signer, run_subject, &positive, &scope).await?;
        check(
            matches!(replay, RunCommandResponse::Started(ref started) if started.execution_id == execution_id)
                && worker.is_alive()?,
            "restoring original assignment policy must restore healthy exact replay",
        )?;
        native_controls(&guest, sentinel, native_target)?;
        let canceled: CancelCommandLaunchResponse =
            signed(&nats, &signer, cancel_subject.clone(), &cancellation, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject, &cancellation, &scope).await?;
        check(
            canceled
                == CancelCommandLaunchResponse::Canceled {
                    execution_id: Some(execution_id),
                }
                && canceled == replay
                && guest.group_members(native_target)?.is_empty()
                && !guest.group_members(sentinel)?.is_empty()
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 2
                && api.control.deletes.load(Ordering::SeqCst) == 0
                && native.rejected.load(Ordering::SeqCst) == 0,
            "restored policy must allow exact key cancellation and preserve the native sentinel without extra effects",
        )?;
        Ok::<_, Boxed>((before_native, after_native, assignment_unchanged, command_unchanged))
    }
    .await;

    let restoration = restore_policy(&context, &original_policy).await;
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
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    restoration?;
    let (before_native, after_native, assignment_unchanged, command_unchanged) = observed?;
    check(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. }))
            && product_removed
            && final_counters[0] == 1
            && final_counters[1] == 1
            && final_counters[2] == 0
            && final_counters[3] == 0
            && final_counters[4] > 0
            && final_counters[5] == 0
            && final_counters[6] == 0
            && final_counters[7] == 0,
        "restored original policy must end in exact targeted physical native cleanup",
    )?;
    check(
        before_native == after_native && assignment_unchanged && command_unchanged,
        &format!("live cached assignment policy refusal must preserve native and durable authority: {policy:?}"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_expiring_assignment_policy_refuses_effects() -> Result<(), Boxed> {
    policy_refusal(Policy::Expiring).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_evicting_assignment_policy_refuses_effects() -> Result<(), Boxed> {
    policy_refusal(Policy::Evicting).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_discard_new_refuses_new_reservation_and_preserves_replay() -> Result<(), Boxed> {
    policy_refusal(Policy::Full).await
}
