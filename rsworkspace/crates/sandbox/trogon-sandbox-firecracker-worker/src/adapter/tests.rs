use trogon_sandbox_contract::OciImageRef;

use super::*;
use crate::adapter::test_support::{TestResult, request};
use crate::config::{BaseRootfs, FirecrackerBinary, KernelImage, StateDir};
use crate::process::DirectLauncher;

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
async fn targeted_cleanup_fences_late_dispatch_and_survives_restart() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let rootfs = tmp.path().join("base.ext4");
    tokio::fs::write(&rootfs, b"x").await?;
    let state = tmp.path().join("state");
    let first = provider(&state, "false", &rootfs);
    let worker_id = first.dispatch_ledger().await?.worker_id().clone();
    let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
        lease_id: LeaseId::new("late"),
        provider: ProviderKind::Firecracker,
        target: trogon_sandbox_contract::DispatchTarget {
            epoch: None,
            worker_id: worker_id.clone(),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("attempt-a")?,
        },
    };
    let cleanup = trogon_sandbox_contract::ProviderCleanupRequest {
        identity: identity.clone(),
        intent: trogon_sandbox_contract::CancelIntent::Release,
    };
    assert!(matches!(
        first.cleanup_dispatch(&cleanup).await,
        trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { .. }
    ));
    drop(first);
    let second = provider(&state, "false", &rootfs);
    assert_eq!(second.dispatch_ledger().await?.worker_id(), &worker_id);
    let dispatch = trogon_sandbox_contract::ProviderDispatchPayload {
        lease_id: identity.lease_id,
        request: request(IMAGE)?,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(identity.target),
    };
    assert!(
        second
            .lease_dispatch(&dispatch)
            .await
            .err()
            .ok_or("the call must fail")?
            .to_string()
            .contains("cancelled")
    );
    assert!(!state.join("late").exists());
    Ok(())
}

#[tokio::test]
async fn a_failed_targeted_boot_can_acknowledge_exact_cleanup() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let rootfs = tmp.path().join("base.ext4");
    tokio::fs::write(&rootfs, b"x").await?;
    let state = tmp.path().join("state");
    let provider = provider(&state, "false", &rootfs);
    let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
        lease_id: LeaseId::new("failed"),
        provider: ProviderKind::Firecracker,
        target: trogon_sandbox_contract::DispatchTarget {
            epoch: None,
            worker_id: provider.dispatch_ledger().await?.worker_id().clone(),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("attempt-a")?,
        },
    };
    let dispatch = trogon_sandbox_contract::ProviderDispatchPayload {
        lease_id: identity.lease_id.clone(),
        request: request(IMAGE)?,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(identity.target.clone()),
    };
    assert!(provider.lease_dispatch(&dispatch).await.is_err());
    let cleanup = trogon_sandbox_contract::ProviderCleanupRequest {
        identity,
        intent: trogon_sandbox_contract::CancelIntent::Release,
    };
    assert!(matches!(
        provider.cleanup_dispatch(&cleanup).await,
        trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { .. }
    ));
    let purge = trogon_sandbox_contract::ProviderCleanupRequest {
        intent: trogon_sandbox_contract::CancelIntent::Purge,
        ..cleanup
    };
    assert!(matches!(
        provider.cleanup_dispatch(&purge).await,
        trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
            intent: trogon_sandbox_contract::CancelIntent::Purge,
            ..
        }
    ));
    assert!(!state.join("failed").exists());
    assert_eq!(provider.current_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_restarted_legacy_attempt_without_resource_proof_stays_unconfirmed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    let identity = {
        let first = provider(&state, "false", &tmp.path().join("base"));
        let ledger = first.dispatch_ledger().await?;
        let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
            lease_id: LeaseId::new("legacy-unknown"),
            provider: ProviderKind::Firecracker,
            target: trogon_sandbox_contract::DispatchTarget {
                worker_id: ledger.worker_id().clone(),
                attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("legacy-unknown-attempt")?,
                epoch: None,
            },
        };
        let result = ledger
            .dispatch(&identity, || async {
                ledger
                    .record_backend_receipt(&identity, lease_of(identity.lease_id.as_str(), &identity.lease_id)?)
                    .await?;
                anyhow::bail!("legacy allocation outcome is unknown")
            })
            .await;
        assert!(result.is_err());
        identity
    };
    let second = provider(&state, "false", &tmp.path().join("base"));
    let response = second
        .cleanup_dispatch(&trogon_sandbox_contract::ProviderCleanupRequest {
            identity: identity.clone(),
            intent: trogon_sandbox_contract::CancelIntent::Purge,
        })
        .await;
    assert!(
        matches!(&response, trogon_sandbox_contract::ProviderCleanupResponse::Failed { identity: actual, .. } if actual == &identity),
        "map and directory absence must not fabricate exact completion: {response:?}"
    );
    assert!(!second.holds_lease(&identity.lease_id).await);
    assert!(!state.join(identity.lease_id.as_str()).exists());
    let states = second.dispatch_ledger().await?.try_states().await?;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].identity, identity);
    assert_eq!(states[0].phase, trogon_sandbox_protocol::AttemptPhase::CleanupRequested);
    Ok(())
}

#[tokio::test]
async fn targeted_spec_refusal_keeps_positive_no_allocation_cleanup() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let ledger = provider.dispatch_ledger().await?;
    let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
        lease_id: LeaseId::new("unsupported-target"),
        provider: ProviderKind::Firecracker,
        target: trogon_sandbox_contract::DispatchTarget {
            worker_id: ledger.worker_id().clone(),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("unsupported-target-attempt")?,
            epoch: Some(trogon_sandbox_contract::LeaseEpoch::new(7)?),
        },
    };
    let error = provider
        .lease_dispatch(&trogon_sandbox_contract::ProviderDispatchPayload {
            lease_id: identity.lease_id.clone(),
            request: request("other:2")?,
            correlation_id: None,
            resolved_drives: Vec::new(),
            target: Some(identity.target.clone()),
        })
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(
        matches!(
            error.downcast_ref::<ProviderError>(),
            Some(ProviderError::InvalidSpec(_))
        ),
        "original refusal is preserved: {error:#}"
    );
    let response = provider
        .cleanup_dispatch(&trogon_sandbox_contract::ProviderCleanupRequest {
            identity: identity.clone(),
            intent: trogon_sandbox_contract::CancelIntent::Purge,
        })
        .await;
    assert!(
        matches!(response, trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { identity: actual, intent: trogon_sandbox_contract::CancelIntent::Purge } if actual == identity)
    );
    assert!(ledger.try_states().await?.is_empty());
    assert!(provider.resources.read_optional(&identity).await?.is_none());
    assert!(!tmp.path().join(identity.lease_id.as_str()).exists());
    Ok(())
}

#[tokio::test]
async fn targeted_capacity_refusal_retains_an_explicit_no_effect_receipt_until_cleanup() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let ledger = provider.dispatch_ledger().await?;
    let _held_capacity = provider
        .slots
        .clone()
        .try_acquire_many_owned(provider.config.max_concurrent_leases)?;
    let identity = trogon_sandbox_contract::LeaseAttemptIdentity {
        lease_id: LeaseId::new("capacity-target"),
        provider: ProviderKind::Firecracker,
        target: trogon_sandbox_contract::DispatchTarget {
            worker_id: ledger.worker_id().clone(),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("capacity-target-attempt")?,
            epoch: Some(trogon_sandbox_contract::LeaseEpoch::new(7)?),
        },
    };
    let error = provider
        .lease_dispatch(&trogon_sandbox_contract::ProviderDispatchPayload {
            lease_id: identity.lease_id.clone(),
            request: request(IMAGE)?,
            correlation_id: None,
            resolved_drives: Vec::new(),
            target: Some(identity.target.clone()),
        })
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<ProviderError>(),
            Some(ProviderError::AtCapacity { .. })
        )),
        "original capacity refusal is preserved: {error:#}"
    );
    let proof = provider.resources.read(&identity).await?;
    assert!(
        !proof.effects_started && !proof.settled,
        "refusal carries durable positive pre-effect evidence"
    );
    let response = provider
        .cleanup_dispatch(&trogon_sandbox_contract::ProviderCleanupRequest {
            identity: identity.clone(),
            intent: trogon_sandbox_contract::CancelIntent::Purge,
        })
        .await;
    assert!(
        matches!(response, trogon_sandbox_contract::ProviderCleanupResponse::Cleaned { identity: actual, intent: trogon_sandbox_contract::CancelIntent::Purge } if actual == identity)
    );
    assert!(ledger.try_states().await?.is_empty());
    assert!(provider.resources.read_optional(&identity).await?.is_none());
    assert!(!tmp.path().join(identity.lease_id.as_str()).exists());
    Ok(())
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
async fn an_exited_vmm_releases_capacity_and_its_directory() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    let provider = provider(&state, "false", &tmp.path().join("base"));
    let lease_id = LeaseId::new("dead");
    let dir = VmDir::for_lease(&provider.config.state_dir, &lease_id)?;
    dir.create().await?;
    let mut process = provider.launcher.launch(&dir).await?;
    assert!(matches!(
        process
            .wait_for_api_socket(&dir.api_socket(), std::time::Duration::from_secs(2))
            .await,
        Err(ProviderError::VmmExited(_))
    ));
    let agent = GuestAgent::new(VsockEndpoint::new(dir.vsock_socket(), provider.config.agent_port));
    let permit = Arc::clone(&provider.slots).try_acquire_owned()?;
    provider.active.lock().await.insert(
        lease_id.as_str().to_owned(),
        ActiveVm {
            resource: None,
            generation: 0,
            _disk_space: None,
            snapshot_policy: None,
            network: None,
            image_defaults: None,
            dir: dir.clone(),
            process,
            agent,
            sandbox_id: lease_id.as_str().to_owned(),
            memory: provider.config.default_memory,
            _permit: permit,
        },
    );

    assert_eq!(provider.current_lease_count().await, 0);
    assert!(!provider.holds_lease(&lease_id).await);
    assert_eq!(provider.slots.available_permits(), 8);
    assert!(!dir.as_path().exists());
    Ok(())
}

#[tokio::test]
async fn a_second_daemon_cannot_clean_the_first_daemons_state() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    let first = provider(&state, "false", &tmp.path().join("base"));
    assert_eq!(first.clean_stale_count().await, 0);
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&state, &alias)?;
    let second = provider(&alias, "false", &tmp.path().join("base"));
    tokio::fs::create_dir_all(state.join("held")).await?;
    tokio::fs::write(state.join("held/rootfs.ext4"), b"live").await?;
    assert_eq!(second.clean_stale_count().await, 0);
    assert!(state.join("held/rootfs.ext4").exists());
    drop(first);
    assert_eq!(second.recover_stale().await?, 1);
    assert!(!state.join("held").exists());
    Ok(())
}

struct RefusingCleanupLauncher;

#[async_trait]
impl VmLauncher for RefusingCleanupLauncher {
    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, ProviderError> {
        DirectLauncher::new(FirecrackerBinary::new("false")).launch(dir).await
    }

    async fn cleanup(&self, _dir: &VmDir) -> Result<(), ProviderError> {
        Err(ProviderError::Launch("injected stale cleanup failure".into()))
    }
}

#[tokio::test]
async fn unreaped_stale_resources_prevent_new_leases() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    let rootfs = tmp.path().join("base");
    tokio::fs::write(&rootfs, b"base").await?;
    tokio::fs::create_dir_all(state.join("unreaped")).await?;
    let mut provider = provider(&state, "false", &rootfs);
    provider.launcher = Arc::new(RefusingCleanupLauncher);
    assert_eq!(provider.clean_stale_count().await, 0);
    let error = provider
        .lease_inner(&request(IMAGE)?, &LeaseId::new("new"))
        .await
        .err()
        .ok_or("the call must fail")?;
    assert!(
        matches!(&error, ProviderError::Launch(reason) if reason.contains("stale")),
        "{error}"
    );
    assert!(!state.join("new").exists());
    Ok(())
}

#[tokio::test]
async fn stale_directories_are_cleaned_on_startup() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = tmp.path().join("state");
    tokio::fs::create_dir_all(state.join("old-lease")).await?;
    tokio::fs::write(state.join("old-lease").join("rootfs.ext4"), b"x").await?;
    let provider = provider(&state, "false", &tmp.path().join("b"));

    assert_eq!(provider.clean_stale_count().await, 1);
    assert!(!state.join("old-lease").exists());
    assert_eq!(provider.clean_stale_count().await, 0);
    Ok(())
}

#[cfg(target_os = "linux")]
async fn orphan_process(dir: &VmDir, socket: &std::path::Path) -> anyhow::Result<tokio::process::Child> {
    let child = tokio::process::Command::new("python3")
        .args(["-c", "import signal; signal.pause()", "--api-sock"])
        .arg(socket)
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().ok_or_else(|| anyhow::anyhow!("the child has no pid"))?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let cmdline = tokio::fs::read(format!("/proc/{pid}/cmdline")).await?;
            if cmdline.split(|byte| *byte == 0).any(|arg| arg == b"--api-sock") {
                break anyhow::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    tokio::fs::write(dir.pid_file(), pid.to_string()).await?;
    Ok(child)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn startup_recovery_finds_a_vmm_whose_pid_file_was_never_persisted() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let dir = VmDir::for_lease(&provider.config.state_dir, &LeaseId::new("missing-pid"))?;
    dir.create().await?;
    let mut child = orphan_process(&dir, &dir.api_socket()).await?;
    tokio::fs::remove_file(dir.pid_file()).await?;
    provider.recover_stale().await?;
    let stopped = tokio::time::timeout(std::time::Duration::from_millis(100), child.wait()).await;
    child.kill().await.ok();
    assert!(stopped.is_ok(), "PID-file loss cannot prove no live VMM exists");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn restart_reaps_a_live_vmm_before_removing_its_files() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let dir = VmDir::for_lease(&provider.config.state_dir, &LeaseId::new("old"))?;
    dir.create().await?;
    let mut child = orphan_process(&dir, &dir.api_socket()).await?;

    assert_eq!(provider.clean_stale_count().await, 1);
    tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await??;
    assert!(!dir.as_path().exists());
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn restart_reaps_a_jailer_before_it_enters_its_chroot() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let dir = VmDir::for_lease(&provider.config.state_dir, &LeaseId::new("starting"))?;
    dir.create().await?;
    let mut child = tokio::process::Command::new("python3")
        .args(["-c", "import signal; signal.pause()", "--chroot-base-dir"])
        .arg(dir.as_path().join("jail"))
        .args(["--id", "vm", "--", "--api-sock", "/api.sock"])
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().ok_or("the child has no pid")?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let cmdline = tokio::fs::read(format!("/proc/{pid}/cmdline")).await?;
            if cmdline.split(|byte| *byte == 0).any(|arg| arg == b"--chroot-base-dir") {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok::<(), std::io::Error>(())
    })
    .await??;
    tokio::fs::write(dir.pid_file(), pid.to_string()).await?;
    provider.clean_stale_count().await;
    let stopped = tokio::time::timeout(std::time::Duration::from_millis(100), child.wait()).await;
    child.kill().await.ok();
    assert!(
        stopped.is_ok(),
        "a worker crash during jailer startup must not orphan its process"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn restart_does_not_signal_a_reused_pid_with_a_similar_socket_argument() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let provider = provider(tmp.path(), "false", &tmp.path().join("base"));
    let dir = VmDir::for_lease(&provider.config.state_dir, &LeaseId::new("old"))?;
    dir.create().await?;
    let other_socket = dir.api_socket().with_extension("sock.unrelated");
    let mut child = orphan_process(&dir, &other_socket).await?;

    provider.clean_stale_count().await;
    let status = tokio::time::timeout(std::time::Duration::from_millis(100), child.wait()).await;
    child.kill().await.ok();
    assert!(status.is_err(), "an unrelated process must survive stale lease cleanup");
    Ok(())
}
