#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_preparing_restart_support;
mod native_process_support;
mod worker_process_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, ReadFileRequest, ReadFileResponse, RunCommandRequest,
    RunCommandResponse, SandboxUserName, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, EpochMode, VerbScope, subjects, tenant_scoped_kv_key};

use ensure_support::{Boxed, Context, check};
use native_access_support::NativeAccess;
use native_command_support::{Guest, NativeEnvd};
use native_preparing_restart_support::HeldBootstrapReply;
use native_process_support::{Api, SANDBOX_ID};
use worker_process_support::{Nats, signed};

const WAIT: Duration = Duration::from_secs(8);
const KEY: &str = "preparing-restart-original";
const POSITIVE: &str = "preparing-restart-positive";
const EFFECT: &str = "preparing-restart-forbidden";

/// Sends a signed request and returns the reply subscription without waiting for the reply.
async fn outstanding<T: Serialize>(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: String,
    request: &T,
    scope: &VerbScope,
) -> Result<async_nats::Subscriber, Boxed> {
    let inbox = nats.client.new_inbox();
    let sub = nats.client.subscribe(inbox.clone()).await?;
    let payload = serde_json::to_vec_pretty(request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &payload, scope)?);
    nats.client
        .publish_with_reply_and_headers(subject, inbox, headers, payload.into())
        .await?;
    nats.client.flush().await?;
    Ok(sub)
}

/// How many times the command wrote its marker.
fn effects(guest: &Guest, name: &str) -> Result<usize, Boxed> {
    let text = guest.effect(name)?.unwrap_or_default();
    let text = String::from_utf8(text)?;
    check(
        text.lines().all(|line| line == "launched"),
        "native marker contained unexpected data",
    )?;
    Ok(text.lines().count())
}

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
    .all(|field| before[field] == after[field])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preparing_cancellation_after_worker_loss_stays_pending_while_the_native_bootstrap_is_unbound()
-> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = async_nats::jetstream::new(nats.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let mut held = HeldBootstrapReply::start(&envd.url).await?;
    let native = NativeAccess::start(held.url.clone(), Arc::clone(&api.control)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("preparing-restart-fixture", [79; 32]);
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
    let lease_id = LeaseId::new("preparing-restart-lease");
    let mut assignment = None;
    let positive_effect = guest.path(POSITIVE).display().to_string();
    let forbidden_effect = guest.path(EFFECT).display().to_string();
    let operation = async {
        let heartbeat = tokio::time::timeout(WAIT, heartbeats.next())
            .await?
            .context("actual worker heartbeat absent")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        check(
            worker.is_alive()?
                && heartbeat.supports_lease_epoch.is_supported()
                && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
                && !heartbeat.profile.isolation.supports_command_launch_cancellation.is_supported(),
            "actual worker must keep the strong feature disabled",
        )?;
        let producer = worker.id();
        let target = DispatchTarget {
            worker_id: heartbeat.worker_id.clone(),
            attempt_id: DispatchAttemptId::new("preparing-restart-attempt")?,
            epoch: Some(LeaseEpoch::new(7)?),
        };
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
        assignment = Some((target.clone(), scope.clone()));
        let grant: ProviderLease =
            worker_process_support::dispatch(&nats, &signer, &dispatch, &target.worker_id).await?;
        check(
            grant.sandbox_id.as_str() == SANDBOX_ID
                && grant.worker_id.as_ref() == Some(&target.worker_id)
                && grant.attempt_id.as_ref() == Some(&target.attempt_id)
                && grant.epoch == target.epoch,
            "actual grant changed its exact native assignment",
        )?;
        let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
        let positive: RunCommandRequest = serde_json::from_value(json!({"lease_id":lease_id,"launch_key":"preparing-positive","require_launch_cancellation":true,
            "command":["bash","-c",format!("echo launched >>{positive_effect}; sleep 300 & wait")],"detached":true,"deadline":"lease","run_as_user":"root"}))?;
        let started: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
        let RunCommandResponse::Started(started) = started else {
            return Err("healthy strong actual worker control did not start".into());
        };
        let positive_binding = guest.last_binding()?;
        tokio::time::timeout(WAIT, async {
            while effects(&guest, POSITIVE)? != 1 {
                check(
                    !guest.group_members(positive_binding)?.is_empty(),
                    "positive native command vanished before marker",
                )?;
                tokio::task::yield_now().await;
            }
            Ok::<_, Boxed>(())
        })
        .await??;
        let positive_cancel = CancelCommandLaunchRequest {
            lease_id: lease_id.clone(),
            launch_key: LaunchKey::new("preparing-positive")?,
        };
        let canceled: CancelCommandLaunchResponse =
            signed(&nats, &signer, cancel_subject.clone(), &positive_cancel, &scope).await?;
        let replay: CancelCommandLaunchResponse =
            signed(&nats, &signer, cancel_subject.clone(), &positive_cancel, &scope).await?;
        check(
            canceled == CancelCommandLaunchResponse::Canceled { execution_id: Some(started.execution_id) }
                && replay == canceled
                && guest.group_members(positive_binding)?.is_empty(),
            "healthy exact cancellation/replay control failed",
        )?;
        let sentinel: RunCommandRequest = serde_json::from_value(json!({"lease_id":lease_id,"command":["sleep","300"],"detached":true,"deadline":"lease","run_as_user":"root"}))?;
        let sentinel_started: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        check(matches!(sentinel_started, RunCommandResponse::Started(_)), "ordinary sentinel failed to start")?;
        let sentinel = guest.last_binding()?;
        check(!guest.group_members(sentinel)?.is_empty(), "independent sentinel not alive")?;
        let commands = context.get_key_value("sandbox-e2b-command-custody").await?;
        let command_key = tenant_scoped_kv_key(&dispatch.request.tenant, &serde_json::to_string(&(lease_id.as_str(), KEY))?);
        let original: RunCommandRequest = serde_json::from_value(json!({"lease_id":lease_id,"launch_key":KEY,"require_launch_cancellation":true,
            "command":["bash","-c",format!("echo launched >>{forbidden_effect}; sleep 300 & wait")],"detached":true,"deadline":"lease","run_as_user":"root"}))?;
        held.arm();
        let bootstrap_began = std::time::Instant::now();
        let mut old_run = outstanding(&nats, &signer, run_subject.clone(), &original, &scope).await?;
        let binding = held.reached().await?;
        let captured = guest.last_binding()?;
        let before: Value = serde_json::from_slice(&commands.get(&command_key).await?.context("Preparing custody absent")?)?;
        check(
            before["phase"] == "preparing"
                && before["execution"].is_null()
                && before["cancel_requested"] == false
                && before["owner"]["tenant"] == scope.tenant.as_str()
                && before["owner"]["lease_id"] == scope.lease_id.as_str()
                && before["owner"]["sandbox_id"] == SANDBOX_ID
                && before["owner"]["worker_id"] == target.worker_id.as_str()
                && before["owner"]["attempt_id"] == target.attempt_id.as_str()
                && before["owner"]["epoch"] == 7
                && before["fingerprint"].as_str().is_some_and(|value| value.len() == 64)
                && before["execution_id"].is_string()
                && binding.pid == captured.pid
                && binding.started == captured.started
                && binding.pid != sentinel.pid
                && !guest.group_members(binding)?.is_empty()
                && held.pending()
                && tokio::time::timeout(Duration::from_millis(50), old_run.next()).await.is_err()
                && effects(&guest, EFFECT)? == 0
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 3,
            "held real bootstrap must precede native binding persistence and any user effect",
        )?;
        let cancel = CancelCommandLaunchRequest {
            lease_id: lease_id.clone(),
            launch_key: LaunchKey::new(KEY)?,
        };
        let pending: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &cancel, &scope).await?;
        check(
            matches!(pending, CancelCommandLaunchResponse::Pending { .. }),
            "unbound Preparing native custody must remain unsettled before crash",
        )?;
        let intent: Value = serde_json::from_slice(&commands.get(&command_key).await?.context("signed cancellation lost Preparing custody")?)?;
        check(
            intent["phase"] == "preparing"
                && intent["cancel_requested"] == true
                && immutable(&before, &intent)
                && held.pending()
                && !guest.group_members(binding)?.is_empty(),
            "signed cancellation must retain intent before actual producer loss",
        )?;
        let assignments = context.get_key_value("sandbox-e2b-assignments").await?;
        let assignment_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&(&target.worker_id, &lease_id))?);
        let assignment_bytes = assignments.get(&assignment_key).await?.context("exact assigned ownership absent")?;
        let fence = std::fs::read(ledger.with_extension("fence"))?;
        check(!fence.is_empty(), "required durable fence absent")?;
        let management_gets_before_restart = api.control.gets.load(Ordering::SeqCst);
        worker.kill()?;
        old_run.unsubscribe().await?;
        drop(heartbeats);
        api.control.recovered_access.store(true, Ordering::SeqCst);
        let mut recovered = nats.client.subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b)).await?;
        nats.client.flush().await?;
        worker = worker_process_support::worker_with_epoch_mode(&nats.url, &api.url, &native.url, &ledger, &signer, 10, EpochMode::Require)?;
        let restarted = worker.id();
        let heartbeat = tokio::time::timeout(WAIT, recovered.next())
            .await?
            .context("actual restarted worker did not recover its occupied assignment")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        check(
            worker.is_alive()?
                && restarted != producer
                && heartbeat.worker_id == target.worker_id
                && heartbeat.capacity.current_leases == 1
                && heartbeat.supports_lease_epoch.is_supported()
                && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
                && !heartbeat.profile.isolation.supports_command_launch_cancellation.is_supported(),
            "actual restart must preserve the same occupied assignment with capability disabled",
        )?;
        check(
            api.control.gets.load(Ordering::SeqCst) > management_gets_before_restart,
            "prerequisite: actual restarted worker did not perform a fresh exact native management GET",
        )?;
        let recovered_before_auth = native.recovered.load(Ordering::SeqCst);
        let auth_request = ReadFileRequest {
            lease_id: lease_id.clone(),
            path: positive_effect.clone(),
            as_user: Some(SandboxUserName::new("root")?),
        };
        let auth_read: ReadFileResponse = signed(&nats, &signer, subjects::provider_read_file(ProviderKind::E2b, &lease_id), &auth_request, &scope)
            .await
            .context("prerequisite: signed recovered native auth control did not return")?;
        check(
            matches!(auth_read, ReadFileResponse::Content { data } if data.as_slice() == b"launched\n")
                && native.recovered.load(Ordering::SeqCst) > recovered_before_auth
                && native.rejected.load(Ordering::SeqCst) == 0,
            "prerequisite: recovered worker did not earn the signed rotated-token native read control",
        )?;
        check(
            held.pending() && !guest.group_members(binding)?.is_empty() && effects(&guest, EFFECT)? == 0,
            "held bootstrap expired before recovered cancellation; crash window is inconclusive",
        )?;
        let replayed: RunCommandResponse = signed(&nats, &signer, run_subject, &original, &scope).await?;
        check(matches!(replayed, RunCommandResponse::Denied(_)), "recovered canceled key cannot bootstrap new work")?;
        check(
            held.pending() && !guest.group_members(binding)?.is_empty(),
            "exact original native target must be alive immediately before recovered cancellation",
        )?;
        let recovered_before_cancel = native.recovered.load(Ordering::SeqCst);
        let canceled: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &cancel, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject, &cancel, &scope).await?;
        let cancel_native_delta = native.recovered.load(Ordering::SeqCst).saturating_sub(recovered_before_cancel);
        let settled = matches!(&canceled, CancelCommandLaunchResponse::Canceled { execution_id: Some(id) } if Some(id.as_str()) == before["execution_id"].as_str());
        let exact_absent = guest.group_members(binding)?.is_empty();
        check(
            bootstrap_began.elapsed() < Duration::from_secs(5),
            "prerequisite: recovered cancellation and exact absence observation exceeded the five-second native gate window",
        )?;
        let sentinel_preserved = !guest.group_members(sentinel)?.is_empty();
        let after: Value = serde_json::from_slice(&commands.get(&command_key).await?.context("recovered cancellation lost original authority")?)?;
        let authority_held = immutable(&before, &after)
            && after["cancel_requested"] == true
            && assignments.get(&assignment_key).await?.is_some_and(|bytes| bytes == assignment_bytes)
            && std::fs::read(ledger.with_extension("fence"))? == fence;
        let native_recovery = api.control.gets.load(Ordering::SeqCst) > 0
            && native.recovered.load(Ordering::SeqCst) > 0
            && native.rejected.load(Ordering::SeqCst) == 0
            && api.control.creates.load(Ordering::SeqCst) == 1
            && api.control.connects.load(Ordering::SeqCst) == 0
            && api.control.lists.load(Ordering::SeqCst) == 0
            && api.control.pauses.load(Ordering::SeqCst) == 0
            && api.control.foreign_gets.load(Ordering::SeqCst) == 0
            && api.control.foreign_deletes.load(Ordering::SeqCst) == 0;
        check(
            native_recovery && authority_held && sentinel_preserved,
            "native recovery, authority or sentinel control failed",
        )?;
        check(
            matches!(&canceled, CancelCommandLaunchResponse::Pending { .. })
                && replay == canceled
                && !settled
                && !exact_absent
                && cancel_native_delta == 0
                && after["phase"] == "preparing"
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 3
                && effects(&guest, EFFECT)? == 0,
            "recovered cancellation of an unbound Preparing bootstrap must stay pending without touching the held native process",
        )
    }
    .await;
    let cleanup = if let Some((target, scope)) = &assignment {
        let request = ProviderCleanupRequest {
            identity: LeaseAttemptIdentity {
                lease_id,
                provider: ProviderKind::E2b,
                target: target.clone(),
            },
            intent: CancelIntent::Release,
        };
        let response: Result<ProviderCleanupResponse, Boxed> = signed(
            &nats,
            &signer,
            subjects::worker_cleanup(ProviderKind::E2b, &target.worker_id),
            &request,
            scope,
        )
        .await;
        response.is_ok_and(|response| {
            matches!(response, ProviderCleanupResponse::Cleaned { identity, intent } if identity == request.identity && intent == request.intent)
        })
    } else {
        false
    };
    let product_removed = api.control.product_removed.load(Ordering::SeqCst);
    let stopped = worker.stop();
    let held_settled = held.shutdown().await;
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    stopped?;
    held_settled?;
    check(
        cleanup && product_removed,
        "Preparing oracle requires exact targeted product native404 cleanup without fallback",
    )?;
    operation
}
