use trogon_sandbox_contract::OciImageRef;

use super::*;
use crate::adapter::test_support::{TestResult, request};
use crate::config::{BaseRootfs, FirecrackerBinary, KernelImage, StateDir};
use crate::process::DirectLauncher;
use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use trogon_sandbox_contract::{WorkerAuthorityRequest, WorkerAuthorityResponse};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};

const IMAGE: &str = "guest:1";

fn provider(state: &std::path::Path, binary: &str, rootfs: &std::path::Path) -> FirecrackerProvider {
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(binary),
        KernelImage::new("/k"),
        BaseRootfs::new(rootfs),
        StateDir::new(state),
    );
    config.default_image = OciImageRef::parse_unqualified(IMAGE);
    config.boot_timeout = std::time::Duration::from_millis(300);
    let launcher = Arc::new(DirectLauncher::new(config.binary.clone()));
    FirecrackerProvider::new(config, launcher)
}

#[tokio::test]
async fn a_rejected_spec_creates_nothing_on_disk() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let rootfs = tmp.path().join("base.ext4");
    tokio::fs::write(&rootfs, b"x").await?;
    let state = tmp.path().join("state");
    let provider = provider(&state, "false", &rootfs);

    let error = provider
        .lease_inner(&request("other:2")?, &LeaseId::new("l1"))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(matches!(error, ProviderError::InvalidSpec(_)), "{error}");
    assert!(!state.exists());
    Ok(())
}

#[tokio::test]
async fn a_failed_boot_removes_the_lease_directory_and_frees_the_slot() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let rootfs = tmp.path().join("base.ext4");
    tokio::fs::write(&rootfs, b"x").await?;
    let state = tmp.path().join("state");
    let provider = provider(&state, "false", &rootfs);
    let lease_id = LeaseId::new("l1");

    let error = provider
        .lease_inner(&request(IMAGE)?, &lease_id)
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(matches!(error, ProviderError::VmmExited(_)), "{error}");
    assert!(!state.join("l1").exists());
    assert_eq!(provider.current_lease_count().await, 0);
    assert_eq!(provider.slots.available_permits(), 8);
    Ok(())
}

#[tokio::test]
async fn a_missing_base_rootfs_fails_before_any_process_starts() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    let provider = provider(&state, "false", &tmp.path().join("missing.ext4"));

    let error = provider
        .lease_inner(&request(IMAGE)?, &LeaseId::new("l1"))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(matches!(error, ProviderError::RootfsCopy(_)), "{error}");
    assert!(!state.join("l1").exists());
    Ok(())
}

#[tokio::test]
async fn cancel_and_purge_are_idempotent_for_unknown_leases() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("b"));
    let lease_id = LeaseId::new("nope");
    provider.cancel(&lease_id).await?;
    provider.cancel(&lease_id).await?;
    provider.purge(&lease_id).await?;
    Ok(())
}

#[tokio::test]
async fn stale_directories_are_cleaned_on_startup() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    tokio::fs::create_dir_all(state.join("old-lease")).await?;
    tokio::fs::write(state.join("old-lease").join("rootfs.ext4"), b"x").await?;
    let sink = Arc::new(RecordingLosses::default());
    let provider = provider(&state, "false", &tmp.path().join("b")).with_loss_sink(sink.clone());
    let lease_id = LeaseId::new("old-lease");

    assert_eq!(provider.clean_stale().await, vec![lease_id.clone()]);
    assert!(!state.join("old-lease").exists());
    assert_eq!(
        sink.losses.lock().await.as_slice(),
        &[(lease_id.clone(), LeaseLoss::WorkerRestarted)]
    );
    assert!(matches!(
        provider.access(&lease_id).await,
        LeaseAccess::Lost(LeaseLoss::WorkerRestarted)
    ));
    assert!(provider.clean_stale().await.is_empty());
    Ok(())
}

#[derive(Default)]
struct RecordingLosses {
    losses: Mutex<Vec<(LeaseId, LeaseLoss)>>,
}

#[async_trait]
impl LeaseLossSink for RecordingLosses {
    async fn lost(&self, lease_id: &LeaseId, loss: &LeaseLoss) {
        self.losses.lock().await.push((lease_id.clone(), loss.clone()));
    }
}

/// Holds a lease backed by a `sleep` in place of a VMM, which is enough for everything
/// that only watches the process.
async fn held_lease(provider: &FirecrackerProvider, lease_id: &LeaseId) -> anyhow::Result<(VmDir, u32)> {
    let dir = VmDir::for_lease(&provider.config.state_dir, lease_id)?;
    let assignment = match provider.dispatch_ledger.get() {
        Some(ledger) => ledger
            .try_states()
            .await?
            .into_iter()
            .find(|state| {
                state.identity.lease_id == *lease_id && state.phase == trogon_sandbox_protocol::AttemptPhase::Starting
            })
            .map(|state| state.identity),
        None => None,
    };
    let resource = match assignment {
        Some(identity) => {
            let resource = provider.resources.begin(&identity).await?;
            Some(provider.resources.start_effects(&resource).await?)
        }
        None => None,
    };
    dir.create().await?;
    if let Some(resource) = &resource {
        provider.resources.attach(&dir, resource).await?;
    }
    let process = VmProcess::supervise(
        tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()?,
        crate::process::Signal::Child,
    );
    let pid = process.pid().ok_or_else(|| anyhow::anyhow!("the process has no pid"))?;
    let agent = GuestAgent::new(VsockEndpoint::new(dir.vsock_socket(), provider.config.agent_port));
    let permit = Arc::clone(&provider.slots).try_acquire_owned()?;
    provider
        .track(lease_id, dir.clone(), process, None, agent, permit)
        .await?;
    Ok((dir, pid))
}

fn authority_identity(
    ledger: &trogon_sandbox_protocol::AttemptLedger,
) -> anyhow::Result<trogon_sandbox_contract::LeaseAttemptIdentity> {
    Ok(trogon_sandbox_contract::LeaseAttemptIdentity {
        lease_id: LeaseId::new("authority-firecracker"),
        provider: ProviderKind::Firecracker,
        target: trogon_sandbox_contract::DispatchTarget {
            worker_id: ledger.worker_id().clone(),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("authority-firecracker-attempt")?,
            epoch: Some(trogon_sandbox_contract::LeaseEpoch::new(7)?),
        },
    })
}

#[tokio::test]
async fn authority_cleanup_never_stops_a_replacement_process_with_the_same_lease_id() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("b"));
    let ledger = provider.dispatch_ledger().await?;
    let identity = authority_identity(ledger)?;
    ledger
        .dispatch(&identity, || async {
            held_lease(&provider, &identity.lease_id).await?;
            lease_of(identity.lease_id.as_str(), &identity.lease_id)
        })
        .await?;
    let original_exit = provider.active.lock().await[identity.lease_id.as_str()].process.exit();
    provider.cancel(&identity.lease_id).await?;
    assert!(original_exit.status().is_some());
    let (replacement_dir, _) = held_lease(&provider, &identity.lease_id).await?;
    let replacement_exit = provider.active.lock().await[identity.lease_id.as_str()].process.exit();
    assert!(replacement_exit.status().is_none());
    let response = provider
        .cleanup_dispatch(&trogon_sandbox_contract::ProviderCleanupRequest {
            identity: identity.clone(),
            intent: trogon_sandbox_contract::CancelIntent::Purge,
        })
        .await;
    let replacement_held = provider.holds_lease(&identity.lease_id).await;
    let replacement_running = replacement_exit.status().is_none();
    let replacement_present = replacement_dir.as_path().exists();
    let cleanup = provider.cancel(&identity.lease_id).await;
    let cleanup_settled = cleanup.is_ok() && replacement_exit.status().is_some() && !replacement_dir.as_path().exists();
    assert!(cleanup_settled, "owned replacement cleanup must settle");
    assert!(
        replacement_held && replacement_running && replacement_present,
        "late authority cleanup stopped its successor: held={replacement_held}, running={replacement_running}, present={replacement_present}, response={response:?}"
    );
    assert!(
        matches!(&response, trogon_sandbox_contract::ProviderCleanupResponse::Failed { identity: actual, .. } if actual == &identity)
            || matches!(&response, trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { identity: actual, intent: trogon_sandbox_contract::CancelIntent::Purge } if actual == &identity),
        "cleanup must refuse or confirm the exact original assignment and intent"
    );
    Ok(())
}

async fn retained_nonce_cleanup(mode: u8) -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("b"));
    let ledger = provider.dispatch_ledger().await?;
    let identity = authority_identity(ledger)?;
    ledger
        .dispatch(&identity, || async {
            held_lease(&provider, &identity.lease_id).await?;
            lease_of(identity.lease_id.as_str(), &identity.lease_id)
        })
        .await?;
    let original = provider.resources.read(&identity).await?;
    provider.cancel(&identity.lease_id).await?;
    let (replacement_dir, _) = held_lease(&provider, &identity.lease_id).await?;
    let replacement_exit = provider.active.lock().await[identity.lease_id.as_str()].process.exit();
    let depot = provider.depot().await?;
    let policy = trogon_sandbox_contract::SnapshotPolicy {
        artifact_type: trogon_sandbox_contract::ArtifactType::FirecrackerSnapshotV1,
        target_repository: "snapshots/nonce-proof".into(),
        retention: None,
        prewarm: None,
        tag_name: None,
    };
    let deadline = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    match mode {
        0 => depot.arm(&identity.lease_id, &policy, 2 * 1024 * 1024, deadline)?,
        1 => {
            let mut foreign = identity.clone();
            foreign.target.attempt_id = trogon_sandbox_contract::DispatchAttemptId::new("foreign-retained-attempt")?;
            let foreign = provider.resources.begin(&foreign).await?;
            let foreign = provider.resources.start_effects(&foreign).await?;
            depot.arm_owned(&identity.lease_id, &policy, 2 * 1024 * 1024, deadline, Some(&foreign))?;
        }
        _ => depot.arm_owned(&identity.lease_id, &policy, 2 * 1024 * 1024, deadline, Some(&original))?,
    }
    depot
        .retain(
            &identity.lease_id,
            crate::snapshot::LocalSnapshot::fixture(tmp.path(), 1)?,
        )
        .await?;
    let snapshot_dir = depot.directory(&identity.lease_id);
    let before = crate::snapshot::directory_info(&snapshot_dir).await?.bytes;
    let before_memory = std::fs::read(snapshot_dir.join(crate::snapshot::MEMORY_FILE))?;
    assert!(before > 1024 * 1024);
    let prior_resource = depot
        .get(&identity.lease_id)?
        .ok_or_else(|| anyhow::anyhow!("the snapshot was not retained"))?
        .resource;
    let request = trogon_sandbox_contract::ProviderCleanupRequest {
        identity: identity.clone(),
        intent: trogon_sandbox_contract::CancelIntent::Purge,
    };
    let response = provider.cleanup_dispatch(&request).await;
    let replacement_held = provider.holds_lease(&identity.lease_id).await;
    let replacement_running = replacement_exit.status().is_none();
    let snapshot_kept = depot.get(&identity.lease_id)?;
    let snapshot_exists = snapshot_dir.exists();
    let memory_preserved =
        snapshot_exists && std::fs::read(snapshot_dir.join(crate::snapshot::MEMORY_FILE))? == before_memory;
    let bytes = if snapshot_exists {
        Some(crate::snapshot::directory_info(&snapshot_dir).await?.bytes)
    } else {
        None
    };
    if snapshot_kept.is_some() {
        depot.mark_purged(&identity.lease_id).await?;
        depot.delete_purged(&identity.lease_id)?;
    }
    let cleanup_original = provider.cleanup_dispatch(&request).await;
    let cleanup_replacement = provider.cancel(&identity.lease_id).await;
    assert!(
        matches!(
            cleanup_original,
            trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { .. }
        ) && cleanup_replacement.is_ok()
            && replacement_exit.status().is_some()
            && !replacement_dir.as_path().exists()
            && !snapshot_dir.exists(),
        "all owned process/snapshot fixtures must settle before assertion"
    );
    assert!(
        replacement_held && replacement_running,
        "snapshot cleanup touched the replacement VM: {response:?}"
    );
    if mode < 2 {
        assert!(
            matches!(&response, trogon_sandbox_contract::ProviderCleanupResponse::Failed { identity: actual, .. } if actual == &identity)
                && snapshot_exists
                && bytes == Some(before)
                && memory_preserved
                && snapshot_kept.as_ref().map(|record| &record.resource) == Some(&prior_resource),
            "mode {mode} deleted an unconfirmed retained allocation: response={response:?}, exists={snapshot_exists}, bytes={bytes:?}"
        );
    } else {
        assert!(
            matches!(&response, trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { identity: actual, intent: trogon_sandbox_contract::CancelIntent::Purge } if actual == &identity)
                && snapshot_kept.is_none()
                && !snapshot_exists,
            "matching retained nonce must be physically purged: response={response:?}, exists={snapshot_exists}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn retained_nonce_cleanup_preserves_legacy_unbound_snapshot_and_replacement_vm() -> TestResult {
    retained_nonce_cleanup(0).await?;
    Ok(())
}

#[tokio::test]
async fn retained_nonce_cleanup_preserves_a_different_snapshot_allocation_and_replacement_vm() -> TestResult {
    retained_nonce_cleanup(1).await?;
    Ok(())
}

#[tokio::test]
async fn retained_nonce_cleanup_deletes_the_matching_snapshot_without_stopping_replacement_vm() -> TestResult {
    retained_nonce_cleanup(2).await?;
    Ok(())
}

#[tokio::test]
async fn authority_worker_reclaims_its_exact_process_after_a_positive_heartbeat() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let provider = Arc::new(provider(tmp.path(), "false", &tmp.path().join("b")));
    let ledger = provider.dispatch_ledger().await?.clone();
    let identity = authority_identity(&ledger)?;
    ledger
        .dispatch(&identity, || async {
            held_lease(&provider, &identity.lease_id).await?;
            lease_of(identity.lease_id.as_str(), &identity.lease_id)
        })
        .await?;
    let (dir, exit) = {
        let held = provider.active.lock().await;
        let vm = &held[identity.lease_id.as_str()];
        (vm.dir.clone(), vm.process.exit())
    };
    let nats = trogon_nats::test_support::JetStreamTestServer::start().await;
    let client = nats.client().await;
    let owner = ledger.worker_id().clone();
    let mut heartbeats = client
        .subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat(
            ProviderKind::Firecracker,
            &owner,
        ))
        .await?;
    let mut queries = client
        .subscribe(trogon_sandbox_protocol::subjects::worker_authority_check(&owner))
        .await?;
    client.flush().await?;
    let checks = Arc::new(AtomicUsize::new(0));
    let (confirm_heartbeat, heartbeat_confirmed) = tokio::sync::oneshot::channel::<()>();
    let oracle = tokio::spawn({
        let client = client.clone();
        let checks = checks.clone();
        let owner = owner.clone();
        async move {
            if heartbeat_confirmed.await.is_err() {
                return;
            }
            while let Some(message) = queries.next().await {
                let Ok(query) = serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload) else {
                    return;
                };
                assert_eq!(query.worker_id, owner);
                checks.fetch_add(1, Ordering::SeqCst);
                let response = WorkerAuthorityResponse {
                    owned: Vec::new(),
                    not_assigned: query.lease_ids,
                };
                let (Some(reply), Ok(payload)) = (message.reply, serde_json::to_vec(&response)) else {
                    return;
                };
                if client.publish(reply, payload.into()).await.is_err() {
                    return;
                }
            }
        }
    });
    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(10);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 1;
    authority.grace = Duration::ZERO;
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut worker = tokio::spawn(crate::worker::run_with_authority(
        client.clone(),
        async_nats::jetstream::new(client),
        provider.clone(),
        owner.clone(),
        cancel.clone(),
        authority,
    ));
    let observation = async {
        let heartbeat = tokio::time::timeout(Duration::from_secs(5), heartbeats.next()).await?;
        let heartbeat: trogon_sandbox_contract::WorkerHeartbeat = serde_json::from_slice(
            &heartbeat
                .ok_or_else(|| anyhow::anyhow!("heartbeat feed closed"))?
                .payload,
        )?;
        anyhow::ensure!(
            heartbeat.worker_id == owner && heartbeat.capacity.current_leases == 1,
            "worker must confirm the positive held allocation"
        );
        let _ = confirm_heartbeat.send(());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let settled = !provider.holds_lease(&identity.lease_id).await
                    && exit.status().is_some()
                    && !dir.as_path().exists()
                    && ledger.try_states().await?.is_empty();
                if settled {
                    break anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|error| {
            anyhow::Error::new(error).context(format!(
                "Firecracker retained its epoch-bearing process after readiness; authority_checks={}",
                checks.load(Ordering::SeqCst)
            ))
        })??;
        anyhow::ensure!(checks.load(Ordering::SeqCst) > 0);
        anyhow::Ok(())
    }
    .await;
    cancel.cancel();
    if tokio::time::timeout(Duration::from_secs(5), &mut worker).await.is_err() {
        worker.abort();
        let _ = worker.await;
    }
    oracle.abort();
    let _ = oracle.await;
    provider.shutdown().await;
    let cleanup_settled =
        exit.status().is_some() && !provider.holds_lease(&identity.lease_id).await && !dir.as_path().exists();
    anyhow::ensure!(cleanup_settled, "owned process and directory cleanup must settle");
    observation.map_err(|error| error.context("owned process and directory cleanup settled=true"))
}

async fn kill(pid: u32) -> anyhow::Result<()> {
    tokio::process::Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .await?;
    Ok(())
}

async fn eventually(mut done: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done().await {
        assert!(tokio::time::Instant::now() < deadline, "condition never held");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_vm_that_dies_mid_lease_is_released_and_reported() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let sink = Arc::new(RecordingLosses::default());
    let provider = provider(tmp.path(), "false", &tmp.path().join("b")).with_loss_sink(sink.clone());
    let lease_id = LeaseId::new("l1");
    let (dir, pid) = held_lease(&provider, &lease_id).await?;
    assert!(matches!(provider.access(&lease_id).await, LeaseAccess::Live(_)));

    kill(pid).await?;
    eventually(async || !sink.losses.lock().await.is_empty()).await;

    let losses = sink.losses.lock().await.clone();
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].0, lease_id);
    assert!(matches!(losses[0].1, LeaseLoss::VmExited { .. }), "{:?}", losses[0].1);
    assert!(matches!(
        provider.access(&lease_id).await,
        LeaseAccess::Lost(LeaseLoss::VmExited { .. })
    ));
    assert!(provider.answers_for(&lease_id).await);
    assert!(!dir.as_path().exists());
    assert_eq!(provider.current_lease_count().await, 0);
    assert_eq!(provider.slots.available_permits(), 8);

    provider.cancel(&lease_id).await?;
    assert!(matches!(provider.access(&lease_id).await, LeaseAccess::NotHeld));
    Ok(())
}

#[tokio::test]
async fn a_cancelled_vm_is_not_reported_as_lost() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let sink = Arc::new(RecordingLosses::default());
    let provider = provider(tmp.path(), "false", &tmp.path().join("b")).with_loss_sink(sink.clone());
    let lease_id = LeaseId::new("l1");
    let (dir, _) = held_lease(&provider, &lease_id).await?;

    provider.cancel(&lease_id).await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert!(sink.losses.lock().await.is_empty());
    assert!(matches!(provider.access(&lease_id).await, LeaseAccess::NotHeld));
    assert!(!dir.as_path().exists());
    Ok(())
}

#[tokio::test]
async fn the_exit_of_a_replaced_vm_does_not_release_its_successor() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let sink = Arc::new(RecordingLosses::default());
    let provider = provider(tmp.path(), "false", &tmp.path().join("b")).with_loss_sink(sink.clone());
    let lease_id = LeaseId::new("l1");
    held_lease(&provider, &lease_id).await?;
    provider.cancel(&lease_id).await?;
    let (_, pid) = held_lease(&provider, &lease_id).await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert!(matches!(provider.access(&lease_id).await, LeaseAccess::Live(_)));
    assert!(sink.losses.lock().await.is_empty());
    kill(pid).await?;
    eventually(async || sink.losses.lock().await.len() == 1).await;
    Ok(())
}
