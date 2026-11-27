#![cfg(unix)]

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use anyhow::{Context, ensure};
use futures::{FutureExt, StreamExt};
use trogon_sandbox_contract::{
    Digest, DispatchAttemptId, DispatchTarget, LeaseEpoch, LeaseId, NativeHandleResponse, ProviderDispatchPayload,
    ProviderDispatchReply, ProviderKind, SandboxImage, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::DispatchSigner;

#[path = "targeted_process_support/snapshot.rs"]
mod snapshot;
mod targeted_process_support;
mod targeted_warm_support;

use targeted_process_support::{Native, Nats, fresh_id};
use targeted_warm_support::{BirthPin, Worker};

fn request(
    lease: &LeaseId,
    tenant: &str,
    image: SandboxImage,
    vcpu_millis: Option<u32>,
    target: Option<DispatchTarget>,
) -> anyhow::Result<ProviderDispatchPayload> {
    targeted_process_support::dispatch_payload(tenant, lease, image, vcpu_millis, target)
}

fn assigned_target(worker: &WorkerId, lease: &LeaseId) -> anyhow::Result<DispatchTarget> {
    Ok(DispatchTarget {
        worker_id: worker.clone(),
        attempt_id: DispatchAttemptId::new(format!("{}-attempt", lease.as_str())).map_err(anyhow::Error::msg)?,
        epoch: Some(LeaseEpoch::new(2)?),
    })
}

async fn begin_pool_phase(
    nats: &Nats,
    heartbeats: &mut async_nats::Subscriber,
    worker: &WorkerId,
) -> anyhow::Result<SystemTime> {
    let inbox = nats.client.new_inbox();
    let mut barrier = nats.client.subscribe(inbox.clone()).await?;
    nats.client.flush().await?;
    let marker = fresh_id();
    nats.client.publish(inbox, marker.clone().into()).await?;
    nats.client.flush().await?;
    let reply = tokio::time::timeout(Duration::from_secs(5), barrier.next())
        .await?
        .context("warm phase NATS ordering barrier ended")?;
    ensure!(
        reply.payload.as_ref() == marker.as_bytes(),
        "warm phase ordering barrier changed"
    );
    loop {
        match heartbeats.next().now_or_never() {
            Some(Some(message)) => {
                let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
                ensure!(
                    heartbeat.provider == ProviderKind::LocalDocker && heartbeat.worker_id == *worker,
                    "drained warm heartbeat changed actual worker identity"
                );
            }
            Some(None) => anyhow::bail!("owned heartbeat ended before a new warm phase"),
            None => break,
        }
    }
    Ok(SystemTime::now())
}

async fn pooled_heartbeat(
    heartbeats: &mut async_nats::Subscriber,
    worker: &WorkerId,
    digest: &Digest,
    phase_started: SystemTime,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(message) = heartbeats.next().await {
            let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
            ensure!(
                heartbeat.provider == ProviderKind::LocalDocker && heartbeat.worker_id == *worker,
                "warm heartbeat changed its actual worker identity"
            );
            if heartbeat.emitted_at > phase_started && heartbeat.warm_pool_population.get(digest) == Some(&1) {
                return Ok::<_, anyhow::Error>(());
            }
        }
        anyhow::bail!("owned worker heartbeat ended before exact warm pool population");
    })
    .await
    .context("actual single-slot warm pool did not become ready")??;
    Ok(())
}

async fn event_barrier(
    nats: &Nats,
    signer: &DispatchSigner,
    sentinel: &ProviderDispatchPayload,
    pin: &BirthPin,
    effects: &snapshot::NativeEffects,
) -> anyhow::Result<()> {
    let marker = format!("warm-effect-barrier-{}", fresh_id());
    effects.register_barrier(&marker, &pin.id)?;
    targeted_warm_support::command(
        nats,
        signer,
        sentinel,
        &format!("process.stdout.write('{marker}')"),
        marker.as_bytes(),
    )
    .await?;
    effects.wait_for_barrier(&marker).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initial_assigned_snapshot_adopts_the_exact_legacy_warm_native_custody() -> anyhow::Result<()> {
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-initial-warm-{}", fresh_id());
    let mut native = Native::new(owner.clone()).await?;
    let docker = bollard::Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(20));
    let mut effects = snapshot::NativeEffects::start(&docker, owner.clone());
    let mut snapshot: Option<snapshot::SnapshotFixture> = None;
    let mut worker: Option<Worker> = None;
    let mut assigned_for_cleanup: Vec<ProviderDispatchPayload> = vec![];
    let observed = std::panic::AssertUnwindSafe(async {
        snapshot = Some(snapshot::SnapshotFixture::prepare(&docker, &mut native, &owner).await?);
        let snapshot = snapshot.as_mut().context("fixture state not prepared")?;
        let mut heartbeats = nats.client.subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(ProviderKind::LocalDocker)).await?;
        nats.client.flush().await?;
        worker = Some(Worker::start(&nats, &directory.path().join("state"), &owner, &signer, &snapshot.reference.registry)?);
        let message = tokio::time::timeout(Duration::from_secs(15), heartbeats.next()).await?.context("actual warm worker heartbeat absent")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(worker.as_mut().context("fixture state not prepared")?.alive()? && heartbeat.provider == ProviderKind::LocalDocker
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1 && heartbeat.supports_lease_epoch.is_supported(),
            "actual warm fixture did not install signed targeted intake");
        let worker_id = heartbeat.worker_id.clone();
        let legacy_subject = trogon_sandbox_protocol::subjects::provider_dispatch(ProviderKind::LocalDocker);
        let assigned_subject = trogon_sandbox_protocol::subjects::worker_dispatch(ProviderKind::LocalDocker, &worker_id);
        let legacy_subscriptions = nats.subscriptions(&legacy_subject).await?;
        let assigned_subscriptions = nats.subscriptions(&assigned_subject).await?;
        let cleanup_subscriptions = nats.subscriptions(&trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, &worker_id)).await?;
        ensure!(legacy_subscriptions.len() == 1 && assigned_subscriptions.len() == 1 && cleanup_subscriptions.len() == 1
            && legacy_subscriptions[0].cid == assigned_subscriptions[0].cid && assigned_subscriptions[0].cid == cleanup_subscriptions[0].cid,
            "warm controls require one exact owned worker connection");
        ensure!(!heartbeat.warm_pool_population.contains_key(&snapshot.reference.digest), "unique snapshot digest unexpectedly started warm");
        let image = SandboxImage::Snapshot { reference: snapshot.reference.clone(), parent: None };
        let birth_lease = LeaseId::new(format!("warm-birth-{}", fresh_id()));
        native.register_lease(birth_lease.as_str());
        let birth = request(&birth_lease, "warm-birth-tenant", image.clone(), None, None)?;
        let birth_grant = targeted_warm_support::granted(targeted_warm_support::dispatch(&nats, &signer, &legacy_subject, &birth).await?)?;
        native.register_id(birth_grant.sandbox_id.as_str());
        let birth_pin = BirthPin::seal(&docker, &native, &birth_grant, birth_lease.as_str()).await?;
        let imported_id = snapshot.seal_import(&docker).await?;
        ensure!(birth_pin.image == imported_id && snapshot.reads() > 0, "actual cold source did not import the immutable snapshot");
        targeted_warm_support::command(&nats, &signer, &birth, &format!("if(require('fs').readFileSync('/tmp/rollback-snapshot-control','utf8')!=={:?})process.exit(7);process.stdout.write('snapshot-source-control')", snapshot.marker()), b"snapshot-source-control").await?;
        let birth_release_phase = begin_pool_phase(&nats, &mut heartbeats, &worker_id).await?;
        targeted_warm_support::legacy_release(&nats, &signer, &birth).await?;
        pooled_heartbeat(&mut heartbeats, &worker_id, &snapshot.reference.digest, birth_release_phase).await?;
        birth_pin.verify(&docker, &native).await?;
        ensure!(native.owned().await? == vec![birth_pin.id.clone()], "first Legacy recycle did not retain the single exact warm native");

        let legacy_lease = LeaseId::new(format!("warm-legacy-hit-{}", fresh_id()));
        native.register_lease(legacy_lease.as_str());
        let legacy = request(&legacy_lease, "warm-next-tenant", image.clone(), None, None)?;
        let legacy_grant = targeted_warm_support::granted(targeted_warm_support::dispatch(&nats, &signer, &legacy_subject, &legacy).await?)?;
        ensure!(legacy_grant.sandbox_id.as_str() == birth_pin.id, "genuine Legacy positive must be the exact warm hit, never a cold substitute");
        birth_pin.verify(&docker, &native).await?;
        targeted_warm_support::command(&nats, &signer, &legacy, "if(require('fs').existsSync('/tmp/rollback-snapshot-control'))process.exit(8);process.stdout.write('legacy-warm-control')", b"legacy-warm-control").await?;
        let legacy_release_phase = begin_pool_phase(&nats, &mut heartbeats, &worker_id).await?;
        targeted_warm_support::legacy_release(&nats, &signer, &legacy).await?;
        pooled_heartbeat(&mut heartbeats, &worker_id, &snapshot.reference.digest, legacy_release_phase).await?;
        birth_pin.verify(&docker, &native).await?;
        ensure!(native.owned().await? == vec![birth_pin.id.clone()], "Legacy warm-hit recycle changed exact native custody");

        let sentinel_lease = LeaseId::new(format!("warm-sentinel-{}", fresh_id()));
        native.register_lease(sentinel_lease.as_str());
        let sentinel = request(&sentinel_lease, "warm-sentinel-tenant", targeted_process_support::registry_image(), Some(1000), Some(assigned_target(&worker_id, &sentinel_lease)?))?;
        assigned_for_cleanup.push(sentinel.clone());
        let sentinel_grant = targeted_warm_support::granted(targeted_warm_support::dispatch(&nats, &signer, &assigned_subject, &sentinel).await?)?;
        native.register_id(sentinel_grant.sandbox_id.as_str());
        ensure!(sentinel_grant.sandbox_id.as_str() != birth_pin.id, "independent sentinel must have a different canonical native CID");
        let sentinel_pin = BirthPin::seal(&docker, &native, &sentinel_grant, sentinel_lease.as_str()).await?;
        targeted_warm_support::command(&nats, &signer, &sentinel, "process.stdout.write('independent-sentinel-control')", b"independent-sentinel-control").await?;
        event_barrier(&nats, &signer, &sentinel, &sentinel_pin, &effects).await?;
        birth_pin.verify(&docker, &native).await?;
        let before_events = effects.checkpoint()?;
        ensure!(before_events.contains(&(birth_pin.id.clone(), "create".into())) && !before_events.contains(&(birth_pin.id.clone(), "destroy".into()))
            && before_events.contains(&(sentinel_pin.id.clone(), "create".into())), "actual native event positive did not preserve original warm and independent sentinel births");
        eprintln!("initial assigned warm controls: valid_actual_cold_snapshot=true genuine_legacy_same_warm_cid=true recycled_same_image_initpid_startedat=true immutable_original_birth_labels=true distinct_tenant_command_and_reset=true single_slot_pool=true phase_fresh_pooled_heartbeats=true nats_ordering_barriers=true signed_actual_worker=true independent_different_cid_sentinel=true warm_birth_pin_preserved_before_primary=true hosted_registry_acceptance=false actual_warm_pool_profile={}", heartbeat.profile.lifecycle.supports_warm_pool.is_supported());

        let assigned_lease = LeaseId::new(format!("warm-assigned-{}", fresh_id()));
        native.register_lease(assigned_lease.as_str());
        let assigned = request(&assigned_lease, "warm-assigned-tenant", image, None, Some(assigned_target(&worker_id, &assigned_lease)?))?;
        assigned_for_cleanup.push(assigned.clone());
        let before = native.owned().await?.into_iter().collect::<BTreeSet<_>>();
        let reply = targeted_warm_support::dispatch(&nats, &signer, &assigned_subject, &assigned).await?;
        let target = assigned.target.as_ref().context("dispatch carries no target")?;
        let exact_grant = match &reply {
            ProviderDispatchReply::Granted(receipt) => {
                native.register_id(receipt.sandbox_id.as_str());
                receipt.sandbox_id.as_str() == birth_pin.id && receipt.worker_id.as_ref() == Some(&target.worker_id)
                    && receipt.attempt_id.as_ref() == Some(&target.attempt_id) && receipt.epoch == target.epoch
                    && receipt.evidence_subject == trogon_sandbox_protocol::subjects::evidence_subject(&assigned.lease_id)
            }
            _ => false,
        };
        let provenance_refusal = matches!(&reply, ProviderDispatchReply::LaunchFailed { provider_error } if provider_error.contains("does not match its running owned native lease"));
        let outcome = match &reply { ProviderDispatchReply::Granted(_) => "granted", ProviderDispatchReply::LaunchFailed { .. } => "launch_failed", ProviderDispatchReply::DispatchRejected { .. } => "dispatch_rejected", ProviderDispatchReply::NativeRejected { .. } => "native_rejected" };
        let exact_native_pin = birth_pin.verify(&docker, &native).await.is_ok();
        let allocations_unchanged = native.owned().await?.into_iter().collect::<BTreeSet<_>>() == before;
        let replay = targeted_warm_support::dispatch(&nats, &signer, &assigned_subject, &assigned).await?;
        let exact_replay = matches!((&reply, &replay), (ProviderDispatchReply::Granted(first), ProviderDispatchReply::Granted(second)) if first == second);
        let ordinary_current = if exact_grant {
            targeted_warm_support::command(&nats, &signer, &assigned, "process.stdout.write('assigned-warm-control')", b"assigned-warm-control").await.is_ok()
        } else { false };
        let current_handle = if exact_grant {
            matches!(targeted_warm_support::native_handle(&nats, &signer, &assigned).await?, NativeHandleResponse::Available(handle)
                if handle.provider == ProviderKind::LocalDocker && handle.vendor_id == birth_pin.id)
        } else { false };
        let late_old_controls_exercised = exact_grant;
        let mut late_old_effects_preserved = false;
        let mut old_reply_outcomes = vec![];
        if exact_grant {
            let marker_nonce = fresh_id();
            let current_marker = format!("/tmp/current-warm-owner-{marker_nonce}");
            targeted_warm_support::command(&nats, &signer, &assigned,
                &format!("require('fs').writeFileSync({current_marker:?},{marker_nonce:?});process.stdout.write('current-owner-marker-ready')"),
                b"current-owner-marker-ready").await?;
            late_old_effects_preserved = true;
            for (index, prior) in [&birth, &legacy].into_iter().enumerate() {
                let old_subject = trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &prior.lease_id);
                let subscriptions = nats.subscriptions(&old_subject).await?;
                ensure!(subscriptions.len() == 1 && subscriptions[0].cid == legacy_subscriptions[0].cid,
                    "late old command requires the same live actual worker subscription");
                let forbidden = format!("/tmp/forbidden-old-warm-owner-{marker_nonce}-{index}");
                targeted_warm_support::legacy_release(&nats, &signer, prior).await?;
                let old_outcome = targeted_warm_support::old_command_probe(&nats, &signer, prior, &forbidden).await?;
                old_reply_outcomes.push(old_outcome.label());
                let current_proof = targeted_warm_support::command(&nats, &signer, &assigned,
                    &format!("const fs=require('fs');if(fs.readFileSync({current_marker:?},'utf8')!=={marker_nonce:?}||fs.existsSync({forbidden:?}))process.exit(9);process.stdout.write('late-old-effects-absent')"),
                    b"late-old-effects-absent").await.is_ok();
                late_old_effects_preserved &= old_outcome.did_not_acknowledge_execution() && current_proof
                    && birth_pin.verify(&docker, &native).await.is_ok()
                    && sentinel_pin.verify(&docker, &native).await.is_ok();
            }
            let after_old = targeted_warm_support::dispatch(&nats, &signer, &assigned_subject, &assigned).await?;
            late_old_effects_preserved &= matches!((&reply, &after_old),
                (ProviderDispatchReply::Granted(first), ProviderDispatchReply::Granted(second)) if first == second)
                && matches!(targeted_warm_support::native_handle(&nats, &signer, &assigned).await?,
                    NativeHandleResponse::Available(handle) if handle.provider == ProviderKind::LocalDocker && handle.vendor_id == birth_pin.id);
        }
        event_barrier(&nats, &signer, &sentinel, &sentinel_pin, &effects).await?;
        let no_native_replacement = effects.checkpoint()? == before_events;
        let sentinel_preserved = sentinel_pin.verify(&docker, &native).await.is_ok();
        let cleanup = targeted_warm_support::cleanup(&nats, &signer, &assigned, &worker_id).await?;
        let exact_cleanup = targeted_warm_support::exact_cleanup(&cleanup, &assigned);
        let exact_native_absent = native.absent(&birth_pin.id).await?;
        let cleanup_replay = targeted_warm_support::cleanup(&nats, &signer, &assigned, &worker_id).await?;
        let exact_cleanup_replay = cleanup_replay == cleanup && targeted_warm_support::exact_cleanup(&cleanup_replay, &assigned);
        let post_cleanup_refused = !matches!(targeted_warm_support::dispatch(&nats, &signer, &assigned_subject, &assigned).await?, ProviderDispatchReply::Granted(_));
        let sentinel_after_cleanup = sentinel_pin.verify(&docker, &native).await.is_ok();
        let only_sentinel = native.owned().await?.into_iter().collect::<BTreeSet<_>>() == BTreeSet::from([sentinel_pin.id.clone()]);
        event_barrier(&nats, &signer, &sentinel, &sentinel_pin, &effects).await?;
        let mut expected_events = before_events;
        expected_events.push((birth_pin.id.clone(), "destroy".into()));
        let exact_cleanup_event_delta = effects.checkpoint()? == expected_events;
        eprintln!("initial assigned warm primary receipt: assigned_outcome={outcome} immutable_birth_provenance_refused={provenance_refusal} same_pooled_cid_exact_assigned_grant={exact_grant} exact_warm_birth_pin_preserved={exact_native_pin} owner_allocation_set_unchanged={allocations_unchanged} same_assignment_receipt_replay={exact_replay} current_assigned_tenant_native_command={ordinary_current} public_current_native_handle_exact={current_handle} late_old_controls_exercised={late_old_controls_exercised} late_old_cancel_and_command_effects_preserved={late_old_effects_preserved} old_command_reply_outcomes={old_reply_outcomes:?} old_cancel_settlement_ack_claimed=false legacy_dispatch_replay_exercised=false observed_native_create_destroy_unchanged={no_native_replacement} independent_sentinel_preserved={sentinel_preserved} exact_targeted_cleanup={exact_cleanup} exact_pooled_native404={exact_native_absent} exact_cleanup_replay={exact_cleanup_replay} post_cleanup_dispatch_refused={post_cleanup_refused} sentinel_after_cleanup_preserved={sentinel_after_cleanup} only_sentinel_remains={only_sentinel} final_event_barrier=true sole_original_warm_destroy_delta={exact_cleanup_event_delta} raw_ledger_inspection=false restart_acceptance=false");
        Ok::<_, anyhow::Error>(exact_grant && exact_native_pin && allocations_unchanged && exact_replay
            && ordinary_current && current_handle && late_old_controls_exercised && late_old_effects_preserved
            && no_native_replacement && sentinel_preserved && exact_cleanup
            && exact_native_absent && exact_cleanup_replay && post_cleanup_refused && sentinel_after_cleanup
            && only_sentinel && exact_cleanup_event_delta && heartbeat.profile.lifecycle.supports_warm_pool.is_supported())
    }).catch_unwind().await;

    for assigned in assigned_for_cleanup.iter().rev() {
        if let Some(target) = &assigned.target {
            let _ = targeted_warm_support::cleanup(&nats, &signer, assigned, &target.worker_id).await;
        }
    }
    let stopped = worker.as_mut().map(Worker::stop).transpose();
    let reconciled = native.finish().await;
    let snapshot_cleaned = match &mut snapshot {
        Some(snapshot) => snapshot.finish(&docker).await,
        None => Ok(()),
    };
    let effects_stopped = effects.stop().await;
    let state_directory = directory.path().to_owned();
    let state_closed = directory.close();
    stopped?;
    let fallback = reconciled?;
    snapshot_cleaned?;
    effects_stopped?;
    state_closed?;
    ensure!(
        !state_directory.exists(),
        "owned initial-warm state temporary directory remains"
    );
    eprintln!(
        "initial assigned warm physical teardown: exact_worker_stopped=true native_event_task_joined=true state_temporary_directory_absent=true fallback_used={fallback}"
    );
    let accepted = match observed {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("the fixture body panicked"),
    };
    ensure!(
        accepted,
        "initial signed assigned snapshot must adopt the exact proven warm CID and preserve current assignment, replay and cleanup custody"
    );
    ensure!(
        !fallback,
        "physical fixture reconciliation cannot promote initial assigned warm reuse acceptance"
    );
    Ok(())
}
