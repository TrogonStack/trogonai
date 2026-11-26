use super::*;
use crate::{DispatchSigner, DispatchVerifier, EnforcementMode};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Ids {
    tenant: TenantId,
    worker: WorkerId,
    attempt: DispatchAttemptId,
    epoch: LeaseEpoch,
}

fn ids() -> Result<Ids, Box<dyn std::error::Error>> {
    Ok(Ids {
        tenant: TenantId::new("health-tenant")?,
        worker: WorkerId::new("health-worker")?,
        attempt: DispatchAttemptId::new("health-attempt")?,
        epoch: LeaseEpoch::new(7)?,
    })
}

fn target() -> Result<DispatchTarget, Box<dyn std::error::Error>> {
    let ids = ids()?;
    Ok(DispatchTarget {
        worker_id: ids.worker,
        attempt_id: ids.attempt,
        epoch: Some(ids.epoch),
    })
}

fn admit(fence: &mut DispatchFence, lease: &str) -> TestResult {
    let ids = ids()?;
    fence.admit_dispatch(
        &LeaseId::new(lease),
        &ids.tenant,
        Some(&ids.worker),
        Some(&ids.attempt),
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    Ok(())
}

fn check(fence: &DispatchFence, lease: &str) -> TestResult {
    let ids = ids()?;
    fence.check_verb(
        &LeaseId::new(lease),
        &ids.tenant,
        Some(&ids.attempt),
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    Ok(())
}

fn verifier(mode: EnforcementMode, epoch: EpochMode) -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("health-key", [103; 32]);
    Ok(DispatchVerifier::new(mode)
        .with_epoch_mode(epoch)
        .with_key(signer.key_id(), &signer.public_key_b64())?)
}

#[cfg(unix)]
fn line(lease: &str) -> Line {
    Line {
        l: lease.into(),
        t: "health-tenant".into(),
        w: Some("health-worker".into()),
        a: Some("health-attempt".into()),
        c: false,
        e: 7,
        x: false,
    }
}

#[test]
fn effective_signature_and_epoch_enforcement_are_required_for_fresh_launch() -> TestResult {
    for mode in [EnforcementMode::Off, EnforcementMode::Warn] {
        assert_eq!(
            verifier(mode, EpochMode::Require)?.fresh_command_launch_fence(),
            Err(FreshLaunchFenceError::VerificationRequired)
        );
    }
    assert_eq!(
        DispatchVerifier::new(EnforcementMode::Require)
            .with_epoch_mode(EpochMode::Require)
            .fresh_command_launch_fence(),
        Err(FreshLaunchFenceError::VerificationKeysUnavailable)
    );
    for epoch in [EpochMode::Off, EpochMode::Warn] {
        assert_eq!(
            verifier(EnforcementMode::Require, epoch)?.fresh_command_launch_fence(),
            Err(FreshLaunchFenceError::EpochRequired)
        );
    }
    assert_eq!(
        verifier(EnforcementMode::Require, EpochMode::Require)?.fresh_command_launch_fence(),
        Err(FreshLaunchFenceError::JournalUnavailable)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn intact_regular_canonical_journal_earns_only_fresh_launch_health() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let verifier = verifier(EnforcementMode::Require, EpochMode::Require)?.with_fence_file(&path)?;
    assert_eq!(verifier.fresh_command_launch_fence(), Ok(()));
    assert!(
        verifier
            .recovered_dispatch_tenant(&LeaseId::new("unknown"), &target()?)
            .is_none()
    );
    Ok(())
}

#[cfg(not(unix))]
#[test]
fn fresh_launch_file_identity_is_explicitly_unearned_without_platform_support() -> TestResult {
    let directory = tempfile::tempdir()?;
    let verifier =
        verifier(EnforcementMode::Require, EpochMode::Require)?.with_fence_file(&directory.path().join("fence"))?;
    assert_eq!(
        verifier.fresh_command_launch_fence(),
        Err(FreshLaunchFenceError::UnsupportedPlatform)
    );
    Ok(())
}

#[test]
fn malformed_journal_does_not_earn_fresh_health_or_recovered_witness() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    std::fs::write(&path, b"{\n")?;
    let mut fence = DispatchFence::open(&path)?;
    assert_eq!(
        fence.fresh_launch_health(),
        Err(FreshLaunchFenceError::UntrustedJournal)
    );
    admit(&mut fence, "legacy-still-admitted")?;
    assert!(
        fence
            .recovered_dispatch_tenant(&LeaseId::new("legacy-still-admitted"), &target()?)
            .is_none()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn canonical_inode_replacement_refuses_fresh_health_and_original_restoration_recovers_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let retained = directory.path().join("retained");
    {
        let mut fence = DispatchFence::open(&path)?;
        admit(&mut fence, "held")?;
    }
    let fence = DispatchFence::open(&path)?;
    let lease = LeaseId::new("held");
    assert_eq!(
        fence.recovered_dispatch_tenant(&lease, &target()?),
        Some(TenantId::new("health-tenant")?)
    );
    std::fs::rename(&path, &retained)?;
    let error = fence
        .fresh_launch_health()
        .err()
        .ok_or("a moved journal has no identity")?;
    assert_eq!(error, FreshLaunchFenceError::JournalIdentityUnavailable);
    assert!(!error.to_string().contains(&path.display().to_string()));
    std::fs::write(&path, b"")?;
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalChanged));
    check(&fence, "held")?;
    assert_eq!(
        fence.recovered_dispatch_tenant(&lease, &target()?),
        Some(TenantId::new("health-tenant")?)
    );
    std::fs::remove_file(&path)?;
    std::fs::rename(&retained, &path)?;
    assert_eq!(fence.fresh_launch_health(), Ok(()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn canonical_symlink_is_not_regular_file_identity_proof() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let retained = directory.path().join("retained");
    let fence = DispatchFence::open(&path)?;
    std::fs::rename(&path, &retained)?;
    std::os::unix::fs::symlink(&retained, &path)?;
    assert_eq!(
        fence.fresh_launch_health(),
        Err(FreshLaunchFenceError::JournalIdentityUnavailable)
    );
    std::fs::remove_file(&path)?;
    std::fs::rename(&retained, &path)?;
    assert_eq!(fence.fresh_launch_health(), Ok(()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn canonical_directory_is_not_regular_file_identity_proof() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let retained = directory.path().join("retained");
    let fence = DispatchFence::open(&path)?;
    std::fs::rename(&path, &retained)?;
    std::fs::create_dir(&path)?;
    assert_eq!(
        fence.fresh_launch_health(),
        Err(FreshLaunchFenceError::JournalIdentityUnavailable)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn same_inode_truncation_below_last_synced_length_refuses_fresh_health() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let mut fence = DispatchFence::open(&path)?;
    admit(&mut fence, "first")?;
    let first_length = std::fs::metadata(&path)?.len();
    admit(&mut fence, "second")?;
    let journal = fence.journal.as_ref().ok_or("a journal-backed fence")?;
    assert!(journal.minimum_synced_length > first_length);
    let identity = journal.identity;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)?
        .set_len(first_length)?;
    assert!(Some(FileIdentity::of(&std::fs::metadata(&path)?)) == identity);
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalChanged));
    Ok(())
}

#[cfg(unix)]
#[test]
fn real_append_failure_is_permanently_latched_without_revoking_old_checks() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let mut fence = DispatchFence::open(&path)?;
    admit(&mut fence, "held")?;
    fence.journal.as_mut().ok_or("a journal-backed fence")?.file = File::open(&path)?;
    admit(&mut fence, "write-failed")?;
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalIoFailed));
    fence.journal.as_mut().ok_or("a journal-backed fence")?.file = open_private(&path, true)?;
    let before = std::fs::metadata(&path)?.len();
    admit(&mut fence, "later-write-succeeds")?;
    assert!(std::fs::metadata(&path)?.len() > before);
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalIoFailed));
    check(&fence, "held")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn sync_failure_is_permanently_latched_after_later_success() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let mut fence = DispatchFence::open(&path)?;
    admit(&mut fence, "held")?;
    fence.append_with_sync(&line("sync-failed"), |_| {
        Err(std::io::Error::other("fixture sync failure"))
    });
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalIoFailed));
    fence.append(&line("later-sync-succeeds"));
    assert_eq!(fence.fresh_launch_health(), Err(FreshLaunchFenceError::JournalIoFailed));
    check(&fence, "held")?;
    Ok(())
}

#[test]
fn memory_fences_keep_ordinary_admission_and_checks_without_fresh_health() -> TestResult {
    let mut fence = DispatchFence::memory();
    admit(&mut fence, "held")?;
    check(&fence, "held")?;
    assert_eq!(
        fence.fresh_launch_health(),
        Err(FreshLaunchFenceError::JournalUnavailable)
    );
    Ok(())
}

#[test]
fn a_full_table_evicts_the_oldest_lease_to_admit_a_new_one() -> TestResult {
    let mut fence = DispatchFence::memory();
    admit(&mut fence, "oldest")?;
    for index in 0..CAPACITY - 1 {
        admit(&mut fence, &format!("lease-{index}"))?;
    }
    assert_eq!(fence.entries.len(), CAPACITY);
    admit(&mut fence, "newcomer")?;
    assert_eq!(fence.entries.len(), CAPACITY);
    assert!(!fence.entries.contains_key("oldest"));
    assert!(fence.entries.contains_key("newcomer"));
    // The evicted lease is simply unknown again; a verb for it is not refused.
    check(&fence, "oldest")?;
    Ok(())
}

#[test]
fn a_full_table_first_drops_leases_older_than_the_retention_window() -> TestResult {
    let Some(stale) = Instant::now().checked_sub(RETENTION + Duration::from_secs(1)) else {
        return Ok(());
    };
    let mut fence = DispatchFence::memory();
    for index in 0..CAPACITY {
        admit(&mut fence, &format!("lease-{index}"))?;
    }
    for entry in fence.entries.values_mut().take(10) {
        entry.seen = stale;
    }
    admit(&mut fence, "newcomer")?;
    assert_eq!(fence.entries.len(), CAPACITY - 10 + 1);
    Ok(())
}

#[test]
fn cleanup_records_unseen_leases_repeats_quietly_and_refuses_other_attempts() -> TestResult {
    let ids = ids()?;
    let lease = LeaseId::new("l1");
    let other_attempt = DispatchAttemptId::new("other")?;
    let mut fence = DispatchFence::memory();
    fence.admit_cleanup(
        &lease,
        &ids.tenant,
        Some(&ids.attempt),
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    fence.admit_cleanup(
        &lease,
        &ids.tenant,
        Some(&ids.attempt),
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    fence.admit_cleanup(&lease, &ids.tenant, None, None, EpochMode::Require)?;
    // A cleaned lease may be cleaned again for another attempt, and a higher epoch is adopted.
    fence.admit_cleanup(
        &lease,
        &ids.tenant,
        Some(&other_attempt),
        Some(LeaseEpoch::new(9)?),
        EpochMode::Require,
    )?;
    assert!(matches!(
        fence.admit_cleanup(&lease, &ids.tenant, None, Some(ids.epoch), EpochMode::Require),
        Err(DispatchAuthError::StaleEpoch)
    ));
    assert!(matches!(
        fence.admit_cleanup(&lease, &TenantId::new("other")?, None, None, EpochMode::Off),
        Err(DispatchAuthError::LeaseTenantConflict)
    ));

    let live = LeaseId::new("live");
    admit(&mut fence, "live")?;
    assert!(matches!(
        fence.admit_cleanup(&live, &ids.tenant, Some(&other_attempt), None, EpochMode::Off),
        Err(DispatchAuthError::StaleAttempt)
    ));
    Ok(())
}

#[test]
fn a_dispatch_without_an_attempt_adopts_a_new_worker_and_higher_epoch() -> TestResult {
    let ids = ids()?;
    let lease = LeaseId::new("l1");
    let mut fence = DispatchFence::memory();
    fence.admit_dispatch(&lease, &ids.tenant, None, None, None, EpochMode::Require)?;
    fence.admit_dispatch(
        &lease,
        &ids.tenant,
        Some(&ids.worker),
        None,
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    let entry = fence.entries.get("l1").ok_or("the lease is held")?;
    assert_eq!(entry.worker.as_deref(), Some("health-worker"));
    assert_eq!(entry.epoch, 7);
    // The same worker and epoch again changes nothing; another worker is refused while the lease is live.
    fence.admit_dispatch(
        &lease,
        &ids.tenant,
        Some(&ids.worker),
        None,
        Some(ids.epoch),
        EpochMode::Require,
    )?;
    let result = fence.admit_dispatch(
        &lease,
        &ids.tenant,
        Some(&WorkerId::new("w2")?),
        None,
        None,
        EpochMode::Off,
    );
    assert!(
        matches!(result, Err(DispatchAuthError::WrongWorker { .. })),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_cleaned_lease_may_move_to_another_worker_and_attempt() -> TestResult {
    let ids = ids()?;
    let lease = LeaseId::new("l1");
    let mut fence = DispatchFence::memory();
    admit(&mut fence, "l1")?;
    fence.admit_cleanup(&lease, &ids.tenant, Some(&ids.attempt), None, EpochMode::Off)?;
    let worker = WorkerId::new("w2")?;
    let attempt = DispatchAttemptId::new("a2")?;
    fence.admit_dispatch(
        &lease,
        &ids.tenant,
        Some(&worker),
        Some(&attempt),
        None,
        EpochMode::Require,
    )?;
    let entry = fence.entries.get("l1").ok_or("the lease is held")?;
    assert_eq!(entry.worker.as_deref(), Some("w2"));
    assert_eq!(entry.attempt.as_deref(), Some("a2"));
    assert!(!entry.cleaned);
    Ok(())
}

#[test]
fn command_readiness_needs_the_exact_live_assignment() -> TestResult {
    let ids = ids()?;
    let lease = LeaseId::new("l1");
    let mut fence = DispatchFence::memory();
    let result = fence.check_command_readiness(&lease, &ids.tenant, &ids.worker, &ids.attempt, ids.epoch);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    admit(&mut fence, "l1")?;
    fence.check_command_readiness(&lease, &ids.tenant, &ids.worker, &ids.attempt, ids.epoch)?;
    let result = fence.check_command_readiness(&lease, &TenantId::new("other")?, &ids.worker, &ids.attempt, ids.epoch);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    let result = fence.check_command_readiness(&lease, &ids.tenant, &WorkerId::new("w2")?, &ids.attempt, ids.epoch);
    assert!(
        matches!(result, Err(DispatchAuthError::WrongWorker { .. })),
        "{result:?}"
    );
    let result = fence.check_command_readiness(
        &lease,
        &ids.tenant,
        &ids.worker,
        &DispatchAttemptId::new("a2")?,
        ids.epoch,
    );
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let result = fence.check_command_readiness(&lease, &ids.tenant, &ids.worker, &ids.attempt, LeaseEpoch::new(8)?);
    assert!(matches!(result, Err(DispatchAuthError::StaleEpoch)), "{result:?}");
    fence.admit_cleanup(&lease, &ids.tenant, Some(&ids.attempt), None, EpochMode::Off)?;
    let result = fence.check_command_readiness(&lease, &ids.tenant, &ids.worker, &ids.attempt, ids.epoch);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    Ok(())
}

#[test]
fn forgetting_an_unknown_lease_writes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence");
    let mut fence = DispatchFence::open(&path)?;
    fence.forget(&LeaseId::new("never"));
    assert_eq!(std::fs::metadata(&path)?.len(), 0);
    admit(&mut fence, "held")?;
    fence.forget(&LeaseId::new("held"));
    assert_eq!(std::fs::read_to_string(&path)?.lines().count(), 2);
    Ok(())
}
