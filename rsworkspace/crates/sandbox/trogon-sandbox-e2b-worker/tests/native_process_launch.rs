#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_command_support;
mod native_process_support;
mod worker_process_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects};

use ensure_support::{Boxed, Context};
use native_command_support::{Guest, NativeEnvd};
use native_process_support::{Api, SANDBOX_ID};
use worker_process_support::{Nats, signed};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_production_worker_cancels_a_signed_strong_launch_by_its_retained_key() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    async_nats::jetstream::new(nats.client.clone()).query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("production-launch-fixture", [41; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = worker_process_support::worker_with_epoch_mode(
        &nats.url,
        &api.url,
        &url::Url::parse(&envd.url)?,
        &ledger,
        &signer,
        10,
        EpochMode::Require,
    )?;
    let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
        .await?
        .context("the actual production worker did not become ready")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    assert!(
        worker.is_alive()?,
        "the actual worker must remain live after its heartbeat"
    );
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("production-launch-attempt")?,
        epoch: Some(LeaseEpoch::FIRST),
    };
    let lease_id = LeaseId::new("production-launch-lease");
    let request = authority_support::lease_request()?;
    let scope = VerbScope {
        tenant: request.tenant.clone(),
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
        "the actual production grant must retain the exact native assignment"
    );
    let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
    let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
    let observed = async {
        let sentinel: RunCommandRequest = serde_json::from_value(json!({ "lease_id":lease_id, "command":["sleep","60"], "detached":true,"deadline":"lease","run_as_user":"root" }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        assert!(matches!(response, RunCommandResponse::Started(_)), "ordinary signed native positive control failed: {response:?}");
        let sentinel = guest.last_binding()?;
        assert!(!guest.group_members(sentinel)?.is_empty() && guest.bootstrap_calls.load(Ordering::SeqCst) == 1, "the actual production ordinary path must establish exactly one live native group");
        let request: RunCommandRequest = serde_json::from_value(json!({ "lease_id":lease_id,"launch_key":"production-owned-start","require_launch_cancellation":true,"command":["bash","-c","sleep 60 & wait"],"detached":true,"deadline":"lease","run_as_user":"root" }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &request, &scope).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err(format!("the initial strong production handler must establish native custody before cancellation: response={response:?}").into());
        };
        let execution_id = started.execution_id;
        let target = guest.last_binding()?;
        assert!(target.pid != sentinel.pid && !guest.group_members(target)?.is_empty(), "the strong positive control must establish a distinct exact native group");
        let request = CancelCommandLaunchRequest { lease_id: lease_id.clone(), launch_key: LaunchKey::new("production-owned-start")? };
        let canceled: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &request, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &request, &scope).await?;
        assert!(api.control.deletes.load(Ordering::SeqCst) == 0, "one-command cancellation must not delete the whole native sandbox");
        let remaining = guest.group_members(target)?;
        let unrelated = guest.group_members(sentinel)?;
        let bootstraps = guest.bootstrap_calls.load(Ordering::SeqCst);
        Ok::<_, Boxed>((execution_id, canceled, replay, remaining, unrelated, bootstraps))
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
    let creates = api.control.creates.load(Ordering::SeqCst);
    let deletes = api.control.deletes.load(Ordering::SeqCst);
    let connects = api.control.connects.load(Ordering::SeqCst);
    let lists = api.control.lists.load(Ordering::SeqCst);
    let foreign_gets = api.control.foreign_gets.load(Ordering::SeqCst);
    let foreign_deletes = api.control.foreign_deletes.load(Ordering::SeqCst);
    worker.stop()?;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    let (execution_id, canceled, replay, remaining, unrelated, bootstraps) = observed.with_context(|| {
        format!(
            "owned worker stopped=true; product_native_cleanup={product_removed}; exact fixture cleanup settled=true"
        )
    })?;
    assert!(
        canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(execution_id)
            }
            && canceled == replay
            && remaining.is_empty(),
        "actual production launch-key cancellation must settle and replay its exact native authority: first={canceled:?}, replay={replay:?}, remaining={remaining:?}; exact owned guest cleanup settled=true"
    );
    assert!(
        !unrelated.is_empty() && bootstraps == 2,
        "the native sentinel must survive without another bootstrap"
    );
    assert!(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. }))
            && product_removed
            && creates == 1
            && deletes == 1,
        "explicit final lease cleanup must cause exact physical native absence: cleanup={cleanup:?}, product_removed={product_removed}, creates={creates}, deletes={deletes}"
    );
    assert!(
        connects == 0 && lists == 0 && foreign_gets == 0 && foreign_deletes == 0,
        "command control must not resume, enumerate or access a different native identity"
    );
    Ok(())
}
