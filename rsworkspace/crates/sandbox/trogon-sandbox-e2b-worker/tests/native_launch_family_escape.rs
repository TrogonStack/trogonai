#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_family_escape_support;
mod native_process_support;
mod worker_process_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse,
    WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects, tenant_scoped_kv_key};

use ensure_support::{Boxed, Context};
use native_access_support::NativeAccess;
use native_command_support::{Guest, NativeEnvd};
use native_process_support::{Api, SANDBOX_ID};
use worker_process_support::{Nats, signed};

const LAUNCH_KEY: &str = "production-native-family-escape";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strong_cancellation_does_not_reach_an_environment_empty_new_session_child_so_the_profile_does_not_claim_it()
-> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = async_nats::jetstream::new(nats.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let native = NativeAccess::start(envd.url.clone(), Arc::clone(&api.control)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("native-family-fixture", [97; 32]);
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
        .context("the actual Require worker did not establish readiness")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    assert!(
        heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
            && !heartbeat
                .profile
                .isolation
                .supports_command_launch_cancellation
                .is_supported(),
        "native counterexample must use the actual assigned worker without enabling its unearned profile"
    );
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("native-family-attempt")?,
        epoch: Some(LeaseEpoch::new(7)?),
    };
    let lease_id = LeaseId::new("native-family-lease");
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
    assert!(
        grant.sandbox_id.as_str() == SANDBOX_ID
            && grant.worker_id.as_ref() == Some(&target.worker_id)
            && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == target.epoch,
        "the native counterexample must hold an exact signed native grant"
    );

    let observed = async {
        let subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
        let sentinel: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"command":["sleep","300"],"detached":true,"deadline":"lease","run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, subject.clone(), &sentinel, &scope).await?;
        assert!(
            matches!(response, RunCommandResponse::Started(_)),
            "independent native sentinel must start"
        );
        let sentinel = guest.last_binding()?;
        let strong: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"command":native_family_escape_support::command(&guest),"env":{},
            "launch_key":LAUNCH_KEY,"require_launch_cancellation":true,"detached":true,
            "deadline":"lease","run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, subject, &strong, &scope).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err("the actual strong native command did not start".into());
        };
        let execution_id = started.execution_id;
        let original = guest.last_binding()?;
        assert!(
            original.pid != sentinel.pid,
            "native target and sentinel must be independent"
        );
        let escaped = native_family_escape_support::wait(&guest, original).await?;
        assert!(
            !guest.group_members(original)?.is_empty()
                && !guest.group_members(sentinel)?.is_empty()
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
            "the escaped-child positive control must retain original target and independent sentinel"
        );
        let assignments = context.get_key_value("sandbox-e2b-assignments").await?;
        let assignment_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&(&target.worker_id, &lease_id))?);
        let assignment = assignments
            .get(&assignment_key)
            .await?
            .context("missing exact durable assignment")?;
        let commands = context.get_key_value("sandbox-e2b-command-custody").await?;
        let command_key = tenant_scoped_kv_key(
            &dispatch.request.tenant,
            &serde_json::to_string(&(lease_id.as_str(), LAUNCH_KEY))?,
        );
        let before: Value =
            serde_json::from_slice(&commands.get(&command_key).await?.context("missing strong custody")?)?;
        assert!(
            before["phase"] == "running" && before["owner"]["sandbox_id"] == SANDBOX_ID,
            "strong native command must have durable exact Running custody before cancellation"
        );
        let fence = std::fs::read(ledger.with_extension("fence"))?;
        let request = CancelCommandLaunchRequest {
            lease_id: lease_id.clone(),
            launch_key: LaunchKey::new(LAUNCH_KEY)?,
        };
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
        let canceled: CancelCommandLaunchResponse =
            signed(&nats, &signer, cancel_subject.clone(), &request, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject, &request, &scope).await?;
        let settled = canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(execution_id),
            };
        let replay_same = replay == canceled;
        let escaped_after = native_family_escape_support::observe(escaped, original)?;
        native_family_escape_support::kill(escaped)?;
        let original_absent = guest.group_members(original)?.is_empty();
        let sentinel_preserved = !guest.group_members(sentinel)?.is_empty();
        let after: Value =
            serde_json::from_slice(&commands.get(&command_key).await?.context("cancellation lost custody")?)?;
        assert!(
            assignments
                .get(&assignment_key)
                .await?
                .is_some_and(|bytes| bytes == assignment)
                && std::fs::read(ledger.with_extension("fence"))? == fence
                && ["owner", "execution_id", "fingerprint", "execution"]
                    .into_iter()
                    .all(|field| after[field] == before[field])
                && after["phase"] == "canceled"
                && after["cancel_requested"] == true,
            "cancel/replay must preserve immutable original assignment and command authority"
        );
        assert!(
            api.control.creates.load(Ordering::SeqCst) == 1
                && api.control.deletes.load(Ordering::SeqCst) == 0
                && api.control.connects.load(Ordering::SeqCst) == 0
                && api.control.lists.load(Ordering::SeqCst) == 0
                && api.control.foreign_gets.load(Ordering::SeqCst) == 0
                && api.control.foreign_deletes.load(Ordering::SeqCst) == 0
                && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
            "strong cancellation cannot replace native custody or disturb unrelated resources"
        );
        Ok::<_, Boxed>((
            settled,
            replay_same,
            escaped_after.alive,
            original_absent,
            sentinel_preserved,
        ))
    }
    .await;

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
    worker.stop()?;
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    let (settled, replay_same, escaped_alive, original_absent, sentinel_preserved) = observed.with_context(|| {
        format!("workers stopped; product_native_cleanup={product_removed}; exact native fixture cleanup settled")
    })?;
    assert!(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. })) && product_removed,
        "the final exact targeted product cleanup must establish native VM/container absence"
    );
    assert!(
        settled && replay_same && original_absent && sentinel_preserved,
        "original native group cancellation and replay must settle while preserving the sentinel"
    );
    assert!(
        escaped_alive,
        "the escaped child is outside the launch family, which is why the profile must not claim launch cancellation"
    );
    Ok(())
}
