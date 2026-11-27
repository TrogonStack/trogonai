#![cfg(unix)]

#[path = "targeted_process_support/authority.rs"]
mod authority;
use authority::*;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, ensure};
use futures::{FutureExt, StreamExt};
use serde_json::Value;
use trogon_sandbox_contract::{
    CancelIntent, LeaseId, ProviderDispatchPayload, ProviderKind, SandboxImage, TenantId, WorkerHeartbeat,
};
use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchReply, ProviderLease, WorkerDispatchProtocol, WorkerId,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope};

mod targeted_process_support;
use targeted_process_support::{Native, Nats, fresh_id};

#[path = "targeted_process_support/snapshot.rs"]
mod rollback_snapshot;

fn payload(lease: &LeaseId, target: Option<DispatchTarget>) -> anyhow::Result<ProviderDispatchPayload> {
    sized_payload(lease, targeted_process_support::registry_image(), None, target)
}

fn sized_payload(
    lease: &LeaseId,
    image: SandboxImage,
    vcpu_millis: Option<u32>,
    target: Option<DispatchTarget>,
) -> anyhow::Result<ProviderDispatchPayload> {
    targeted_process_support::dispatch_payload("docker-process-intake", lease, image, vcpu_millis, target)
}

async fn dispatch(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: &str,
    payload: &ProviderDispatchPayload,
) -> anyhow::Result<ProviderDispatchReply> {
    let body = signer
        .sign_payload(&serde_json::to_vec(payload)?, OP_LEASE_DISPATCH)
        .map_err(anyhow::Error::msg)?;
    let message = nats
        .client
        .send_request(
            subject.to_owned(),
            async_nats::Request::new()
                .payload(body.into())
                .timeout(Some(Duration::from_secs(15))),
        )
        .await?;
    serde_json::from_slice(&message.payload).context("worker reply did not match the public dispatch contract")
}

fn granted(reply: ProviderDispatchReply) -> anyhow::Result<ProviderLease> {
    match reply {
        ProviderDispatchReply::Granted(grant) => Ok(grant),
        ProviderDispatchReply::LaunchFailed { provider_error } => {
            eprintln!(
                "targeted intake provider refusal: error_class={}",
                diagnostic_class(&provider_error)
            );
            anyhow::bail!("actual worker refused native allocation")
        }
        ProviderDispatchReply::DispatchRejected { .. } => {
            anyhow::bail!("actual worker rejected dispatch authentication")
        }
        ProviderDispatchReply::NativeRejected { .. } => {
            anyhow::bail!("actual worker rejected native options")
        }
    }
}

fn diagnostic_class(message: &str) -> &'static str {
    for (needle, class) in [
        ("actual worker refused native allocation", "native_allocation_refused"),
        (
            "actual worker rejected dispatch authentication",
            "dispatch_authentication_rejected",
        ),
        ("actual worker rejected native options", "native_options_rejected"),
        (
            "TargetedV1 and epoch claims",
            "targeted_claim_or_same_connection_mismatch",
        ),
        ("targeted cleanup must use the same", "cleanup_connection_mismatch"),
        ("native grant lost its exact", "grant_assignment_mismatch"),
        ("grant is not its exact running", "native_grant_identity_mismatch"),
        ("allocation set", "native_allocation_set_changed"),
        ("replay the exact first", "replayed_grant_changed"),
        (
            "stale or foreign signed assignment",
            "stale_or_foreign_dispatch_granted",
        ),
        ("bypassed targeted ownership", "assigned_legacy_bypass_granted"),
        (
            "stale or foreign targeted cleanup",
            "stale_or_foreign_cleanup_not_rejected",
        ),
        ("did not acknowledge the exact", "cleanup_identity_mismatch"),
        ("resurrected native ownership", "cleaned_dispatch_granted"),
        ("created another container", "cleaned_dispatch_reallocated"),
        ("product cleanup did not confirm", "product_cleanup_absence_unconfirmed"),
        ("running owned native lease", "provider_native_ownership_mismatch"),
        (
            "allocated Docker lease has no registration",
            "provider_native_registration_missing",
        ),
        (
            "changed its admitted request or native receipt",
            "provider_native_registration_changed",
        ),
        ("not canonical", "provider_native_receipt_noncanonical"),
        ("ready attempt has no receipt", "ledger_ready_receipt_missing"),
        (
            "backend receipt has no started attempt",
            "ledger_backend_attempt_missing",
        ),
        (
            "backend receipt identity or phase mismatch",
            "ledger_backend_identity_or_phase_mismatch",
        ),
        ("dispatch attempt identity was reused", "ledger_attempt_identity_reused"),
        ("UNIQUE constraint failed", "ledger_unique_constraint_failed"),
        ("database is locked", "ledger_database_locked"),
        ("no responders", "no_responders"),
        ("timed out", "timeout"),
        ("deadline has elapsed", "deadline_elapsed"),
    ] {
        if message.contains(needle) {
            return class;
        }
    }
    "unclassified_redacted"
}

fn headers(
    signer: &DispatchSigner,
    subject: &str,
    bytes: &[u8],
    dispatch: &ProviderDispatchPayload,
) -> anyhow::Result<async_nats::HeaderMap> {
    let scope = VerbScope {
        tenant: dispatch.request.tenant.clone(),
        lease_id: dispatch.lease_id.clone(),
        worker_id: dispatch.target.as_ref().map(|target| target.worker_id.clone()),
        attempt_id: dispatch.target.as_ref().map(|target| target.attempt_id.clone()),
        epoch: dispatch.target.as_ref().and_then(|target| target.epoch),
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(subject, bytes, &scope)?);
    Ok(headers)
}

async fn cleanup(
    nats: &Nats,
    signer: &DispatchSigner,
    dispatch: &ProviderDispatchPayload,
    recipient: &WorkerId,
) -> anyhow::Result<Value> {
    let target = dispatch.target.as_ref().context("targeted cleanup needs a target")?;
    let subject = trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, recipient);
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let bytes = serde_json::to_vec(&request)?;
    let message = nats
        .client
        .send_request(
            subject.clone(),
            async_nats::Request::new()
                .headers(headers(signer, &subject, &bytes, dispatch)?)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(15))),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

async fn legacy_cleanup(
    nats: &Nats,
    signer: &DispatchSigner,
    dispatch: &ProviderDispatchPayload,
) -> anyhow::Result<()> {
    let subject = trogon_sandbox_protocol::subjects::provider_cancel(ProviderKind::LocalDocker, &dispatch.lease_id);
    nats.client
        .publish_with_headers(
            subject.clone(),
            headers(signer, &subject, &[], dispatch)?,
            Vec::new().into(),
        )
        .await?;
    nats.client.flush().await?;
    Ok(())
}

async fn retained(native: &Native, grant: &ProviderLease, lease: &LeaseId) -> anyhow::Result<()> {
    native.running(grant.sandbox_id.as_str(), lease.as_str()).await?;
    ensure!(
        native.owned().await? == vec![grant.sandbox_id.as_str().to_owned()],
        "replay or rejection changed the actual native allocation set"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_worker_adopts_exact_signed_targeted_assignment_once() -> anyhow::Result<()> {
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-process-intake-{}", fresh_id());
    let mut native = Native::new(owner.clone()).await?;
    let legacy_lease = LeaseId::new("docker-process-intake-legacy");
    let targeted_lease = LeaseId::new("docker-process-intake-assigned");
    native.register_lease(legacy_lease.as_str());
    native.register_lease(targeted_lease.as_str());
    let mut heartbeats = nats
        .client
        .subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(
            ProviderKind::LocalDocker,
        ))
        .await?;
    nats.client.flush().await?;
    let mut worker = targeted_process_support::worker(&nats, &directory.path().join("state"), &owner, &signer)?;
    let observation = async {
        let message = tokio::time::timeout(Duration::from_secs(15), heartbeats.next()).await?
            .context("the actual owned Docker worker did not publish a heartbeat")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(heartbeat.provider == ProviderKind::LocalDocker && worker.is_alive()?, "heartbeat did not come from the live owned Docker worker");
        let legacy_subject = trogon_sandbox_protocol::subjects::provider_dispatch(ProviderKind::LocalDocker);
        let connection = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                ensure!(worker.is_alive()?, "actual Docker worker exited before intake readiness");
                let subscriptions = nats.subscriptions(&legacy_subject).await?;
                if subscriptions.len() == 1 && subscriptions[0].subject == legacy_subject
                    && subscriptions[0].qgroup.as_deref() == Some("sandbox-provider-local_docker") {
                    return Ok::<_, anyhow::Error>(subscriptions[0].cid);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.context("NATS did not positively confirm the owned worker's legacy intake")??;
        let legacy = payload(&legacy_lease, None)?;
        let legacy_grant = granted(dispatch(&nats, &signer, &legacy_subject, &legacy).await?)?;
        native.register_id(legacy_grant.sandbox_id.as_str());
        native.running(legacy_grant.sandbox_id.as_str(), legacy_lease.as_str()).await?;
        ensure!(legacy_grant.worker_id.as_ref().is_some_and(|worker| *worker == heartbeat.worker_id), "actual legacy allocation and heartbeat disagree on worker identity");
        legacy_cleanup(&nats, &signer, &legacy).await?;
        native.wait_absent(legacy_grant.sandbox_id.as_str()).await?;
        ensure!(native.owned().await?.is_empty(), "legacy positive left native assets behind");
        eprintln!("targeted intake setup: actual_worker_live=true heartbeat_observed=true legacy_subscription_confirmed=true signed_legacy_native_allocation=true exact_legacy_cleanup=true");

        let target = DispatchTarget {
            worker_id: heartbeat.worker_id.clone(),
            attempt_id: DispatchAttemptId::new("docker-process-intake-attempt").map_err(anyhow::Error::msg)?,
            epoch: Some(LeaseEpoch::new(2)?),
        };
        let assigned = payload(&targeted_lease, Some(target.clone()))?;
        let subject = trogon_sandbox_protocol::subjects::worker_dispatch(ProviderKind::LocalDocker, &target.worker_id);
        let subscriptions = nats.subscriptions(&subject).await?;
        let grant = granted(dispatch(&nats, &signer, &subject, &assigned).await
            .context("MISSING PRODUCT FEATURE: the live Docker worker must serve its exact signed TargetedV1 intake after successful signed native legacy allocation and cleanup")?)?;
        native.register_id(grant.sandbox_id.as_str());
        ensure!(heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1 && heartbeat.supports_lease_epoch.is_supported()
            && subscriptions.len() == 1 && subscriptions[0].cid == connection && subscriptions[0].subject == subject,
            "TargetedV1 and epoch claims require installed exact intake on the same actual worker connection");
        let cleanup_subscriptions = nats.subscriptions(&trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, &target.worker_id)).await?;
        ensure!(cleanup_subscriptions.len() == 1 && cleanup_subscriptions[0].cid == connection, "targeted cleanup must use the same actual worker connection");
        ensure!(grant.worker_id.as_ref() == Some(&target.worker_id) && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == Some(LeaseEpoch::new(2)?), "native grant lost its exact signed worker, attempt or nonzero epoch");
        retained(&native, &grant, &targeted_lease).await?;
        let replay = granted(dispatch(&nats, &signer, &subject, &assigned).await?)?;
        ensure!(replay == grant, "same signed assignment must replay the exact first native grant");
        retained(&native, &grant, &targeted_lease).await?;

        let mut stale = assigned.clone();
        stale.target.as_mut().context("fixture state not prepared")?.epoch = Some(LeaseEpoch::FIRST);
        let mut foreign = assigned.clone();
        foreign.target.as_mut().context("fixture state not prepared")?.worker_id = WorkerId::new("foreign-docker-worker").map_err(anyhow::Error::msg)?;
        let mut other_attempt = assigned.clone();
        other_attempt.target.as_mut().context("fixture state not prepared")?.attempt_id = DispatchAttemptId::new("foreign-docker-attempt").map_err(anyhow::Error::msg)?;
        for refused in [&stale, &foreign, &other_attempt] {
            ensure!(!matches!(dispatch(&nats, &signer, &subject, refused).await?, ProviderDispatchReply::Granted(_)), "stale or foreign signed assignment was granted");
            retained(&native, &grant, &targeted_lease).await?;
        }
        ensure!(!matches!(dispatch(&nats, &signer, &legacy_subject, &assigned).await?, ProviderDispatchReply::Granted(_)), "assigned payload bypassed targeted ownership through legacy intake");
        retained(&native, &grant, &targeted_lease).await?;
        for (axis, refused) in [("stale_epoch", &stale), ("foreign_worker", &foreign), ("foreign_attempt", &other_attempt)] {
            let response = cleanup(&nats, &signer, refused, &target.worker_id).await?;
            let outcome = match serde_json::from_value::<ProviderCleanupResponse>(response.clone()) {
                Ok(ProviderCleanupResponse::Cleaned { .. }) => "cleaned",
                Ok(ProviderCleanupResponse::Failed { .. }) => "failed",
                Err(_) if response.get("error").and_then(Value::as_str) == Some("dispatch_auth") => "dispatch_auth_rejected",
                Err(_) if response.get("decode_error").and_then(Value::as_str).is_some() => "decode_rejected",
                Err(_) => "unclassified_redacted",
            };
            eprintln!("targeted intake cleanup diagnostic: axis={axis} typed_outcome={outcome}");
            ensure!(response.get("decode_error").and_then(Value::as_str).is_some()
                || (response.get("error").and_then(Value::as_str) == Some("dispatch_auth")
                    && response.get("message").and_then(Value::as_str).is_some_and(|message| !message.is_empty() && message.len() <= 4096))
                || matches!(serde_json::from_value::<ProviderCleanupResponse>(response), Ok(ProviderCleanupResponse::Failed { .. })),
                "stale or foreign targeted cleanup was not explicitly rejected");
            retained(&native, &grant, &targeted_lease).await?;
        }
        let response: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, &assigned, &target.worker_id).await?)?;
        ensure!(matches!(response, ProviderCleanupResponse::Cleaned { ref identity, intent: CancelIntent::Release }
            if identity.lease_id == targeted_lease && identity.provider == ProviderKind::LocalDocker && identity.target == target),
            "targeted cleanup did not acknowledge the exact signed identity");
        native.wait_absent(grant.sandbox_id.as_str()).await?;
        ensure!(!matches!(dispatch(&nats, &signer, &subject, &assigned).await?, ProviderDispatchReply::Granted(_)), "cleaned assignment replay resurrected native ownership");
        ensure!(native.owned().await?.is_empty(), "cleaned assignment replay created another container");
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Err(error) = &observation {
        for (index, cause) in error.chain().take(8).enumerate() {
            eprintln!(
                "targeted intake primary observation: cause_index={index} error_class={}",
                diagnostic_class(&cause.to_string())
            );
        }
    }
    let stopped = worker.stop();
    let reconciled = native.finish().await;
    stopped?;
    let fallback = reconciled?;
    ensure!(
        !fallback,
        "fixture reconciliation cannot promote native product cleanup to acceptance"
    );
    observation
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assigned_rollback_refuses_before_restoring_or_replacing_exact_native_custody() -> anyhow::Result<()> {
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("state");
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-assigned-rollback-{}", fresh_id());
    let mut native = Native::new(owner.clone()).await?;
    let docker = bollard::Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(20));
    let mut effects = rollback_snapshot::NativeEffects::start(&docker, owner.clone());
    let mut snapshot: Option<rollback_snapshot::SnapshotFixture> = None;
    let mut worker: Option<targeted_process_support::Process> = None;
    let mut grants: Vec<(ProviderDispatchPayload, ProviderLease)> = vec![];
    let observed = std::panic::AssertUnwindSafe(async {
        snapshot = Some(rollback_snapshot::SnapshotFixture::prepare(&docker, &mut native, &owner).await?);
        let snapshot = snapshot.as_mut().context("fixture state not prepared")?;
        let mut heartbeats = nats.client.subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(ProviderKind::LocalDocker)).await?;
        nats.client.flush().await?;
        worker = Some(targeted_process_support::worker_with_registry(&nats, &state, &owner, &signer, &snapshot.reference.registry)?);
        let message = tokio::time::timeout(Duration::from_secs(15), heartbeats.next()).await?
            .context("actual assigned-rollback worker heartbeat absent")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(worker.as_mut().context("fixture state not prepared")?.is_alive()? && heartbeat.provider == ProviderKind::LocalDocker
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1 && heartbeat.supports_lease_epoch.is_supported(),
            "actual rollback worker did not install signed assigned intake");
        let worker_id = heartbeat.worker_id.clone();
        let legacy_subject = trogon_sandbox_protocol::subjects::provider_dispatch(ProviderKind::LocalDocker);
        let assigned_subject = trogon_sandbox_protocol::subjects::worker_dispatch(ProviderKind::LocalDocker, &worker_id);
        let legacy_subscriptions = nats.subscriptions(&legacy_subject).await?;
        let assigned_subscriptions = nats.subscriptions(&assigned_subject).await?;
        let cleanup_subscriptions = nats.subscriptions(&trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, &worker_id)).await?;
        ensure!(legacy_subscriptions.len() == 1 && assigned_subscriptions.len() == 1 && cleanup_subscriptions.len() == 1
            && legacy_subscriptions[0].cid == assigned_subscriptions[0].cid && assigned_subscriptions[0].cid == cleanup_subscriptions[0].cid,
            "rollback controls must use one actual assigned worker connection");
        let cold_lease = LeaseId::new(format!("rollback-cold-{}", fresh_id()));
        native.register_lease(cold_lease.as_str());
        let cold = sized_payload(&cold_lease, SandboxImage::Snapshot { reference: snapshot.reference.clone(), parent: None }, Some(1000), None)?;
        let cold_grant = granted(dispatch(&nats, &signer, &legacy_subject, &cold).await?)?;
        native.register_id(cold_grant.sandbox_id.as_str());
        grants.push((cold.clone(), cold_grant.clone()));
        native.running(cold_grant.sandbox_id.as_str(), cold.lease_id.as_str()).await?;
        snapshot_command_control(&nats, &signer, &cold, snapshot.marker()).await?;
        let imported_id = snapshot.seal_import(&docker).await?;
        ensure!(docker.inspect_container(cold_grant.sandbox_id.as_str(), None).await?.image.as_ref() == Some(&imported_id)
            && snapshot.reads() > 0, "cold snapshot positive did not import and execute its actual immutable rootfs");
        legacy_cleanup(&nats, &signer, &cold).await?;
        native.wait_absent(cold_grant.sandbox_id.as_str()).await?;

        let legacy_lease = LeaseId::new(format!("rollback-legacy-{}", fresh_id()));
        native.register_lease(legacy_lease.as_str());
        let legacy = sized_payload(&legacy_lease, targeted_process_support::registry_image(), Some(1000), None)?;
        let legacy_grant = granted(dispatch(&nats, &signer, &legacy_subject, &legacy).await?)?;
        native.register_id(legacy_grant.sandbox_id.as_str());
        grants.push((legacy.clone(), legacy_grant.clone()));
        ordinary_cleanup_control(&nats, &signer, &legacy).await?;
        let before_legacy = snapshot.reads();
        let response: trogon_sandbox_contract::SandboxRollbackResponse = rollback_rpc(&nats, &signer, &legacy,
            trogon_sandbox_protocol::subjects::provider_rollback(ProviderKind::LocalDocker, &legacy.lease_id),
            &trogon_sandbox_contract::SandboxRollbackRequest { lease_id: legacy.lease_id.clone(), snapshot: snapshot.reference.clone() }).await?;
        ensure!(response == trogon_sandbox_contract::SandboxRollbackResponse::Done, "genuine Legacy rollback positive did not complete");
        let legacy_replacement = rollback_native_handle(&nats, &signer, &legacy).await?;
        native.register_id(&legacy_replacement);
        native.running(&legacy_replacement, legacy.lease_id.as_str()).await?;
        ensure!(legacy_replacement != legacy_grant.sandbox_id.as_str() && native.absent(legacy_grant.sandbox_id.as_str()).await?
            && snapshot.reads() > before_legacy, "Legacy rollback positive did not replace the actual old container from the valid snapshot");
        snapshot_command_control(&nats, &signer, &legacy, snapshot.marker()).await?;
        ensure!(docker.inspect_container(&legacy_replacement, None).await?.image.as_ref() == Some(&imported_id), "Legacy rollback positive changed snapshot provenance");
        legacy_cleanup(&nats, &signer, &legacy).await?;
        native.wait_absent(&legacy_replacement).await?;
        ensure!(native.owned().await?.is_empty(), "valid snapshot and Legacy rollback controls left native assets");
        eprintln!("assigned rollback controls: immutable_digest_rootfs=true actual_cold_snapshot_command=true genuine_legacy_rollback_done=true exact_legacy_native_cleanup=true sized_requests_exclude_warm_pool=true registry_transport=owned_loopback_fixture hosted_registry_acceptance=false actual_heartbeat_in_place_rollback={}", heartbeat.profile.snapshot.supports_in_place_rollback.is_supported());

        let mut assigned_grants = vec![];
        for suffix in ["old", "sentinel"] {
            let lease = LeaseId::new(format!("rollback-{suffix}-{}", fresh_id()));
            native.register_lease(lease.as_str());
            let assigned = sized_payload(&lease, targeted_process_support::registry_image(), Some(1000), Some(DispatchTarget {
                worker_id: worker_id.clone(),
                attempt_id: DispatchAttemptId::new(format!("{}-attempt", lease.as_str())).map_err(anyhow::Error::msg)?,
                epoch: Some(LeaseEpoch::new(2)?),
            }))?;
            let grant = granted(dispatch(&nats, &signer, &assigned_subject, &assigned).await?)?;
            native.register_id(grant.sandbox_id.as_str());
            grants.push((assigned.clone(), grant.clone()));
            ensure!(grant.worker_id.as_ref() == Some(&worker_id)
                && grant.attempt_id.as_ref() == assigned.target.as_ref().map(|target| &target.attempt_id)
                && grant.epoch == Some(LeaseEpoch::new(2)?), "assigned rollback positive lost exact assignment authority");
            ordinary_cleanup_control(&nats, &signer, &assigned).await?;
            assigned_grants.push((assigned, grant));
        }
        let (old, old_grant) = &assigned_grants[0];
        let (sentinel, sentinel_grant) = &assigned_grants[1];
        let old_pin = NativePin::seal(&native, &docker, old_grant, &old.lease_id).await?;
        let sentinel_pin = NativePin::seal(&native, &docker, sentinel_grant, &sentinel.lease_id).await?;
        ensure!(granted(dispatch(&nats, &signer, &assigned_subject, old).await?)? == *old_grant, "healthy assigned rollback replay control changed the native receipt");
        rollback_effect_barrier(&nats, &signer, sentinel, sentinel_grant.sandbox_id.as_str(), &effects).await?;
        let before_effects = effects.checkpoint()?;
        for (id, action) in [(cold_grant.sandbox_id.as_str(), "create"), (cold_grant.sandbox_id.as_str(), "destroy"),
            (legacy_grant.sandbox_id.as_str(), "create"), (legacy_grant.sandbox_id.as_str(), "destroy"),
            (legacy_replacement.as_str(), "create"), (legacy_replacement.as_str(), "destroy"),
            (old_grant.sandbox_id.as_str(), "create"), (sentinel_grant.sandbox_id.as_str(), "create")] {
            ensure!(before_effects.contains(&(id.to_owned(), action.into())), "native effect observer missed actual fixture positive controls");
        }
        let before = native.owned().await?.into_iter().collect::<BTreeSet<_>>();
        let before_reads = snapshot.reads();
        let fence_before = std::fs::read(state.join("dispatch-fence.jsonl"))?;
        let response: trogon_sandbox_contract::SandboxRollbackResponse = rollback_rpc(&nats, &signer, old,
            trogon_sandbox_protocol::subjects::provider_rollback(ProviderKind::LocalDocker, &old.lease_id),
            &trogon_sandbox_contract::SandboxRollbackRequest { lease_id: old.lease_id.clone(), snapshot: snapshot.reference.clone() }).await?;
        let outcome = match response { trogon_sandbox_contract::SandboxRollbackResponse::Done => "done", trogon_sandbox_contract::SandboxRollbackResponse::Unsupported => "unsupported", trogon_sandbox_contract::SandboxRollbackResponse::Denied(_) => "denied" };
        let unsupported = matches!(response, trogon_sandbox_contract::SandboxRollbackResponse::Unsupported);
        let restore_not_started = snapshot.reads() == before_reads;
        let old_absent = native.absent(old_grant.sandbox_id.as_str()).await?;
        let old_preserved = old_pin.verify(&native, &docker, &old.lease_id).await.is_ok();
        let handle = rollback_native_handle(&nats, &signer, old).await;
        let handle_unchanged = handle.as_ref().is_ok_and(|id| id == old_grant.sandbox_id.as_str());
        if let Ok(id) = handle { native.register_id(&id); native.running(&id, old.lease_id.as_str()).await?; }
        let allocation_unchanged = native.owned().await?.into_iter().collect::<BTreeSet<_>>() == before;
        let replay = dispatch(&nats, &signer, &assigned_subject, old).await?;
        let exact_replay = matches!(replay, ProviderDispatchReply::Granted(ref grant) if grant == old_grant);
        let fence_unchanged = std::fs::read(state.join("dispatch-fence.jsonl"))? == fence_before;
        let sentinel_preserved = sentinel_pin.verify(&native, &docker, &sentinel.lease_id).await.is_ok();
        rollback_effect_barrier(&nats, &signer, sentinel, sentinel_grant.sandbox_id.as_str(), &effects).await?;
        let native_mutations_unchanged = effects.checkpoint()? == before_effects;
        let cleanup_response: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, old, &worker_id).await?)?;
        let cleanup_accepted = exact_cleanup(&cleanup_response, old);
        let cleanup_native_absent = native.absent(old_grant.sandbox_id.as_str()).await?;
        let cleanup_replay: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, old, &worker_id).await?)?;
        let cleanup_replayed = cleanup_replay == cleanup_response && exact_cleanup(&cleanup_replay, old);
        let sentinel_after_cleanup = sentinel_pin.verify(&native, &docker, &sentinel.lease_id).await.is_ok();
        eprintln!("assigned rollback primary receipt: rollback_outcome={outcome} valid_snapshot_controls=true restore_not_started={restore_not_started} observed_owned_create_destroy_events_unchanged={native_mutations_unchanged} native_event_positive_and_exact_exec_barriers=true old_native_absent_before_cleanup={old_absent} old_native_pin_preserved={old_preserved} public_native_handle_unchanged={handle_unchanged} exact_allocation_set_unchanged={allocation_unchanged} same_assignment_replay_unchanged={exact_replay} exact_fence_authority_unchanged={fence_unchanged} independent_sentinel_preserved={sentinel_preserved} exact_targeted_cleanup_accepted={cleanup_accepted} old_native_absent_after_cleanup={cleanup_native_absent} exact_cleanup_replayed={cleanup_replayed} sentinel_preserved_after_cleanup={sentinel_after_cleanup} actual_heartbeat_in_place_rollback={}", heartbeat.profile.snapshot.supports_in_place_rollback.is_supported());
        Ok::<_, anyhow::Error>((unsupported, restore_not_started, old_preserved, handle_unchanged, allocation_unchanged,
            exact_replay, fence_unchanged, sentinel_preserved, cleanup_accepted, cleanup_native_absent,
            cleanup_replayed, sentinel_after_cleanup, !heartbeat.profile.snapshot.supports_in_place_rollback.is_supported(), native_mutations_unchanged))
    }).catch_unwind().await;

    for (assigned, _) in grants.iter().rev() {
        if let Some(target) = &assigned.target {
            let _ = cleanup(&nats, &signer, assigned, &target.worker_id).await;
        } else {
            let _ = legacy_cleanup(&nats, &signer, assigned).await;
        }
    }
    let stopped = worker.as_mut().map(targeted_process_support::Process::stop).transpose();
    let reconciled = native.finish().await;
    let snapshot_cleaned = match &mut snapshot {
        Some(snapshot) => snapshot.finish(&docker).await,
        None => Ok(()),
    };
    let effects_stopped = effects.stop().await;
    let state_directory = directory.path().to_owned();
    let state_closed = directory.close();
    if let Ok(Err(error)) = &observed {
        eprintln!(
            "assigned rollback fixture failed: error_class={}",
            diagnostic_class(&error.to_string())
        );
    }
    stopped?;
    let fallback = reconciled?;
    snapshot_cleaned?;
    effects_stopped?;
    state_closed?;
    ensure!(
        !state_directory.exists(),
        "owned rollback state temporary directory remains"
    );
    eprintln!(
        "assigned rollback physical teardown: exact_worker_stopped=true native_event_task_joined=true state_temporary_directory_absent=true fallback_used={fallback}"
    );
    let (
        unsupported,
        restore_not_started,
        old_preserved,
        handle_unchanged,
        allocation_unchanged,
        exact_replay,
        fence_unchanged,
        sentinel_preserved,
        cleanup_accepted,
        cleanup_native_absent,
        cleanup_replayed,
        sentinel_after_cleanup,
        advertised_refusal,
        native_mutations_unchanged,
    ) = match observed {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("the fixture body panicked"),
    };
    ensure!(
        unsupported
            && restore_not_started
            && old_preserved
            && handle_unchanged
            && allocation_unchanged
            && exact_replay
            && fence_unchanged
            && sentinel_preserved
            && cleanup_accepted
            && cleanup_native_absent
            && cleanup_replayed
            && sentinel_after_cleanup,
        "assigned rollback must refuse the valid snapshot before restore or exact native-custody destruction"
    );
    ensure!(
        native_mutations_unchanged,
        "assigned rollback caused owned native create/destroy effects between exact observer barriers"
    );
    ensure!(
        advertised_refusal,
        "assigned-configured worker must not advertise unsupported in-place rollback"
    );
    ensure!(
        !fallback,
        "physical fixture reconciliation cannot promote assigned rollback acceptance"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assigned_lease_cleanup_survives_canonical_fence_loss_without_restoring_fresh_admission() -> anyhow::Result<()>
{
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("state");
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-cleanup-fence-{}", fresh_id());
    let mut native = Native::new(owner.clone()).await?;
    let docker = bollard::Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(10));
    let mut heartbeats = nats
        .client
        .subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(
            ProviderKind::LocalDocker,
        ))
        .await?;
    nats.client.flush().await?;
    let mut worker = targeted_process_support::worker(&nats, &state, &owner, &signer)?;
    let mut grants: Vec<(ProviderDispatchPayload, ProviderLease)> = Vec::new();
    let mut fault: Option<CanonicalFenceFault> = None;
    let observed = std::panic::AssertUnwindSafe(async {
        let message = tokio::time::timeout(Duration::from_secs(15), heartbeats.next()).await?
            .context("actual cleanup-fixture worker heartbeat absent")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(worker.is_alive()? && heartbeat.provider == ProviderKind::LocalDocker
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1 && heartbeat.supports_lease_epoch.is_supported(),
            "actual cleanup fixture did not install healthy assigned intake");
        let worker_id = heartbeat.worker_id.clone();
        let dispatch_subject = trogon_sandbox_protocol::subjects::worker_dispatch(ProviderKind::LocalDocker, &worker_id);
        let dispatch_subscriptions = nats.subscriptions(&dispatch_subject).await?;
        let cleanup_subscriptions = nats.subscriptions(&trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, &worker_id)).await?;
        ensure!(dispatch_subscriptions.len() == 1 && cleanup_subscriptions.len() == 1
            && dispatch_subscriptions[0].cid == cleanup_subscriptions[0].cid,
            "dispatch and cleanup must belong to the same actual worker connection");
        for suffix in ["healthy", "old", "sentinel"] {
            let lease = LeaseId::new(format!("cleanup-fence-{suffix}-{}", fresh_id()));
            native.register_lease(lease.as_str());
            let assigned = payload(&lease, Some(DispatchTarget {
                worker_id: worker_id.clone(),
                attempt_id: DispatchAttemptId::new(format!("{}-attempt", lease.as_str())).map_err(anyhow::Error::msg)?,
                epoch: Some(LeaseEpoch::new(2)?),
            }))?;
            let grant = granted(dispatch(&nats, &signer, &dispatch_subject, &assigned).await?)?;
            native.register_id(grant.sandbox_id.as_str());
            grants.push((assigned, grant));
            let (assigned, grant) = grants.last().context("no grant recorded")?;
            ensure!(grant.worker_id.as_ref() == Some(&worker_id)
                && grant.attempt_id.as_ref() == assigned.target.as_ref().map(|target| &target.attempt_id)
                && grant.epoch == Some(LeaseEpoch::new(2)?), "positive grant changed its exact assignment");
            native.running(grant.sandbox_id.as_str(), assigned.lease_id.as_str()).await?;
        }
        let (healthy, healthy_grant) = grants[0].clone();
        let (old, old_grant) = grants[1].clone();
        let (sentinel, sentinel_grant) = grants[2].clone();
        let old_pin = NativePin::seal(&native, &docker, &old_grant, &old.lease_id).await?;
        let sentinel_pin = NativePin::seal(&native, &docker, &sentinel_grant, &sentinel.lease_id).await?;
        ordinary_cleanup_control(&nats, &signer, &healthy).await?;
        let healthy_cleanup: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, &healthy, &worker_id).await?)?;
        ensure!(exact_cleanup(&healthy_cleanup, &healthy), "healthy assigned lease cleanup positive failed");
        native.wait_absent(healthy_grant.sandbox_id.as_str()).await?;
        let healthy_replay: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, &healthy, &worker_id).await?)?;
        ensure!(healthy_replay == healthy_cleanup, "healthy exact cleanup receipt was not replayable");
        old_pin.verify(&native, &docker, &old.lease_id).await?;
        sentinel_pin.verify(&native, &docker, &sentinel.lease_id).await?;
        eprintln!("assigned lease cleanup positive controls: signed_targeted_epoch=true same_worker_connection=true ordinary_native=true exact_cleanup=true exact_native_absent=true exact_cleanup_replay=true independent_old_and_sentinel=true");
        fault = Some(CanonicalFenceFault::replace(&state.join("dispatch-fence.jsonl"))?);
        let before: BTreeSet<_> = native.owned().await?.into_iter().collect();
        let fresh_lease = LeaseId::new(format!("cleanup-fence-fresh-{}", fresh_id()));
        native.register_lease(fresh_lease.as_str());
        let fresh = payload(&fresh_lease, Some(DispatchTarget {
            worker_id: worker_id.clone(),
            attempt_id: DispatchAttemptId::new(format!("{}-attempt", fresh_lease.as_str())).map_err(anyhow::Error::msg)?,
            epoch: Some(LeaseEpoch::new(2)?),
        }))?;
        let fresh_reply = dispatch(&nats, &signer, &dispatch_subject, &fresh).await?;
        let fresh_refused = !matches!(fresh_reply, ProviderDispatchReply::Granted(_));
        if let ProviderDispatchReply::Granted(grant) = fresh_reply {
            native.register_id(grant.sandbox_id.as_str());
            grants.push((fresh, grant));
        }
        let fresh_effect_absent = native.owned().await?.into_iter().collect::<BTreeSet<_>>() == before;
        let mut stale = old.clone(); stale.target.as_mut().context("fixture state not prepared")?.epoch = Some(LeaseEpoch::FIRST);
        let mut foreign_attempt = old.clone(); foreign_attempt.target.as_mut().context("fixture state not prepared")?.attempt_id = DispatchAttemptId::new("foreign-cleanup-attempt").map_err(anyhow::Error::msg)?;
        let mut foreign_tenant = old.clone(); foreign_tenant.request.tenant = TenantId::new("foreign-cleanup-tenant")?;
        let before_foreign = fault.as_ref().context("fault fixture not installed")?.retained_bytes()?;
        let mut foreign_refused = true;
        for invalid in [&stale, &foreign_attempt, &foreign_tenant] {
            foreign_refused &= cleanup_refused(cleanup(&nats, &signer, invalid, &worker_id).await?);
            old_pin.verify(&native, &docker, &old.lease_id).await?;
            sentinel_pin.verify(&native, &docker, &sentinel.lease_id).await?;
        }
        let before_cleanup = fault.as_ref().context("fault fixture not installed")?.retained_bytes()?;
        let foreign_authority_unchanged = before_foreign == before_cleanup;
        let response: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, &old, &worker_id).await?)?;
        let cleanup_accepted = exact_cleanup(&response, &old);
        let old_absent_under_fault = native.absent(old_grant.sandbox_id.as_str()).await?;
        let replay: ProviderCleanupResponse = serde_json::from_value(cleanup(&nats, &signer, &old, &worker_id).await?)?;
        let exact_cleanup_replayed = exact_cleanup(&replay, &old) && replay == response;
        let sentinel_preserved = sentinel_pin.verify(&native, &docker, &sentinel.lease_id).await.is_ok();
        let cleaned_dispatch_refused = !matches!(dispatch(&nats, &signer, &dispatch_subject, &old).await?, ProviderDispatchReply::Granted(_));
        let fault_present = fault.as_ref().context("fault fixture not installed")?.present()?;
        let exact_tombstone_only = exact_cleanup_tombstone(&before_cleanup,
            &fault.as_ref().context("fault fixture not installed")?.retained_bytes()?, &old);
        let only_sentinel_remains = native.owned().await?.into_iter().collect::<BTreeSet<_>>()
            == BTreeSet::from([sentinel_grant.sandbox_id.as_str().to_owned()]);
        Ok::<_, anyhow::Error>((fresh_refused, fresh_effect_absent, foreign_refused, cleanup_accepted,
            old_absent_under_fault, exact_cleanup_replayed, sentinel_preserved,
            cleaned_dispatch_refused, fault_present, only_sentinel_remains,
            foreign_authority_unchanged, exact_tombstone_only))
    }).catch_unwind().await;
    let restored = match &mut fault {
        Some(fault) => fault.restore_for_physical_cleanup(),
        None => Ok(()),
    };
    let mut physical_cleanup_ok = true;
    for (assigned, grant) in grants.iter().rev() {
        let worker_id = &assigned
            .target
            .as_ref()
            .context("dispatch carries no target")?
            .worker_id;
        let cleaned = match cleanup(&nats, &signer, assigned, worker_id).await {
            Ok(value) => serde_json::from_value::<ProviderCleanupResponse>(value)
                .is_ok_and(|response| exact_cleanup(&response, assigned)),
            Err(_) => false,
        };
        let absent = native.absent(grant.sandbox_id.as_str()).await.unwrap_or(false);
        physical_cleanup_ok &= cleaned && absent;
    }
    let stopped = worker.stop_gracefully().await;
    if stopped.is_err() {
        worker.stop()?;
    }
    let fallback = native.finish().await?;
    restored?;
    stopped?;
    ensure!(
        physical_cleanup_ok && !fallback,
        "physical fixture teardown failed and cannot establish semantic cleanup"
    );
    let (
        fresh_refused,
        fresh_effect_absent,
        foreign_refused,
        cleanup_accepted,
        old_absent_under_fault,
        exact_cleanup_replayed,
        sentinel_preserved,
        cleaned_dispatch_refused,
        fault_present,
        only_sentinel_remains,
        foreign_authority_unchanged,
        exact_tombstone_only,
    ) = match observed {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("the fixture body panicked"),
    };
    eprintln!(
        "assigned lease cleanup stale-fence receipt: fresh_dispatch_refused={fresh_refused} fresh_native_effect_absent={fresh_effect_absent} stale_foreign_cleanup_refused={foreign_refused} foreign_cleanup_authority_unchanged={foreign_authority_unchanged} exact_old_cleanup_accepted={cleanup_accepted} exact_old_native_absent_under_fault={old_absent_under_fault} exact_cleanup_replayed_under_fault={exact_cleanup_replayed} exact_owned_tombstone_only={exact_tombstone_only} sentinel_native_identity_preserved={sentinel_preserved} cleaned_dispatch_refused={cleaned_dispatch_refused} canonical_fault_present_during_semantics={fault_present} only_sentinel_remains_under_fault={only_sentinel_remains} inode_restored_for_physical_teardown_only=true exact_physical_native_cleanup=true fallback_used=false"
    );
    ensure!(
        fresh_refused && fresh_effect_absent && foreign_refused && sentinel_preserved && fault_present,
        "fence-loss controls did not preserve fresh-refusal, ownership, sentinel or canonical fault"
    );
    ensure!(
        foreign_authority_unchanged && exact_tombstone_only,
        "cleanup must preserve foreign authority and append only its exact retained tombstone"
    );
    ensure!(
        cleanup_accepted && old_absent_under_fault && exact_cleanup_replayed && only_sentinel_remains,
        "retained exact assigned lease cleanup must settle and prove native absence while the canonical fence remains unhealthy"
    );
    ensure!(
        cleaned_dispatch_refused,
        "cleaned assignment must not restore fresh native admission"
    );
    Ok(())
}
