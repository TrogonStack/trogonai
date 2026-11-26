#![allow(clippy::mutable_key_type)]

use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;

use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderKind, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityHostError, AuthorityMode, AuthorityModeError, AuthorityResourceId,
    AuthorityResourceIdError, AuthorityStats, AuthorityTaskScope, CONFIRMATIONS, DEFAULT_CHECK_INTERVAL, DEFAULT_GRACE,
    DEFAULT_PARTITION_AFTER, ENV_WORKER_AUTHORITY, FreezeOutcome, HeldLease, MAX_LEASES_PER_REQUEST, MIN_LEASE_AGE,
    OracleError, PurgeOutcome, ThawOutcome,
};

type TestResult = Result<(), Box<dyn Error>>;

fn identity(lease: &str, epoch: Option<LeaseEpoch>) -> Result<LeaseAttemptIdentity, Box<dyn Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new(lease),
        provider: ProviderKind::Fly,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("attempt-1")?,
            epoch,
        },
    })
}

#[test]
fn mode_parses_names_ignoring_case_and_space() -> TestResult {
    assert_eq!(AuthorityMode::parse("off")?, AuthorityMode::Off);
    assert_eq!(AuthorityMode::parse("OFF")?, AuthorityMode::Off);
    assert_eq!(AuthorityMode::parse("  Warn ")?, AuthorityMode::Warn);
    assert_eq!(AuthorityMode::parse("enforce")?, AuthorityMode::Enforce);
    assert_eq!(AuthorityMode::parse("ENFORCE\n")?, AuthorityMode::Enforce);
    Ok(())
}

#[test]
fn empty_mode_means_warn() -> TestResult {
    assert_eq!(AuthorityMode::parse("")?, AuthorityMode::Warn);
    assert_eq!(AuthorityMode::parse("   ")?, AuthorityMode::Warn);
    Ok(())
}

#[test]
fn unknown_mode_is_rejected_with_the_lowercased_value() -> TestResult {
    let error = AuthorityMode::parse("  REQUIRE ")
        .err()
        .ok_or("an unknown mode must be rejected")?;
    assert_eq!(
        error,
        AuthorityModeError {
            found: "require".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "SANDBOX_WORKER_AUTHORITY must be off, warn or enforce, got require"
    );
    Ok(())
}

#[test]
fn mode_from_an_unset_variable_is_warn() -> TestResult {
    assert_eq!(AuthorityMode::from_env_value(None)?, AuthorityMode::Warn);
    Ok(())
}

#[test]
fn mode_from_a_set_variable_is_parsed() -> TestResult {
    assert_eq!(AuthorityMode::from_env_value(Some("enforce"))?, AuthorityMode::Enforce);
    assert_eq!(AuthorityMode::from_env_value(Some(""))?, AuthorityMode::Warn);
    assert!(AuthorityMode::from_env_value(Some("sometimes")).is_err());
    Ok(())
}

#[test]
fn mode_reads_the_process_environment_without_panicking() {
    // The value depends on the surrounding environment; only the call itself is exercised.
    let _ = AuthorityMode::from_env();
}

#[test]
fn mode_implements_from_str() -> TestResult {
    assert_eq!("off".parse::<AuthorityMode>()?, AuthorityMode::Off);
    assert!("nope".parse::<AuthorityMode>().is_err());
    Ok(())
}

#[test]
fn config_defaults_follow_the_published_constants() {
    let config = AuthorityConfig::new(AuthorityMode::Enforce);
    assert_eq!(config.mode, AuthorityMode::Enforce);
    assert_eq!(config.interval, DEFAULT_CHECK_INTERVAL);
    assert_eq!(config.partition_after, DEFAULT_PARTITION_AFTER);
    assert_eq!(config.grace, DEFAULT_GRACE);
    assert_eq!(config.min_lease_age, MIN_LEASE_AGE);
    assert_eq!(config.confirmations, CONFIRMATIONS);
    assert_eq!(config.request_timeout, Duration::from_secs(5));
}

#[test]
fn constants_have_their_documented_values() {
    assert_eq!(ENV_WORKER_AUTHORITY, "SANDBOX_WORKER_AUTHORITY");
    assert_eq!(MAX_LEASES_PER_REQUEST, 10_000);
    assert_eq!(DEFAULT_CHECK_INTERVAL, Duration::from_secs(15));
    assert_eq!(DEFAULT_PARTITION_AFTER, Duration::from_secs(45));
    assert_eq!(DEFAULT_GRACE, Duration::from_secs(60));
    assert_eq!(MIN_LEASE_AGE, Duration::from_secs(30));
    assert_eq!(CONFIRMATIONS, 2);
}

#[test]
fn stats_start_at_zero() {
    let stats = AuthorityStats::default();
    assert_eq!(AuthorityStats::get(&stats.unowned), 0);
    assert_eq!(AuthorityStats::get(&stats.partitioned), 0);
    assert_eq!(AuthorityStats::get(&stats.frozen), 0);
    assert_eq!(AuthorityStats::get(&stats.thawed), 0);
    assert_eq!(AuthorityStats::get(&stats.purged), 0);
    assert_eq!(AuthorityStats::get(&stats.check_failures), 0);
    assert_eq!(AuthorityStats::get(&stats.no_responders), 0);
}

#[test]
fn stats_read_what_was_stored_in_a_counter() {
    let stats = AuthorityStats::default();
    stats.purged.store(7, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(AuthorityStats::get(&stats.purged), 7);
    assert_eq!(AuthorityStats::get(&stats.frozen), 0);
}

#[test]
fn resource_id_keeps_a_valid_value() -> TestResult {
    let id = AuthorityResourceId::new("machine-1")?;
    assert_eq!(id.as_str(), "machine-1");
    assert_eq!(id, AuthorityResourceId::new(String::from("machine-1"))?);
    Ok(())
}

#[test]
fn resource_id_accepts_exactly_the_maximum_length() -> TestResult {
    let id = AuthorityResourceId::new("a".repeat(AuthorityResourceId::MAX_LEN))?;
    assert_eq!(id.as_str().len(), AuthorityResourceId::MAX_LEN);
    Ok(())
}

#[test]
fn resource_id_rejects_empty_oversized_and_nul() -> TestResult {
    let empty = AuthorityResourceId::new("").err().ok_or("empty must be rejected")?;
    assert_eq!(empty, AuthorityResourceIdError::Empty);
    assert_eq!(empty.to_string(), "authority resource identity must not be empty");

    let long = AuthorityResourceId::new("a".repeat(AuthorityResourceId::MAX_LEN + 1))
        .err()
        .ok_or("oversized must be rejected")?;
    assert_eq!(long, AuthorityResourceIdError::TooLong { max: 4096 });
    assert_eq!(
        long.to_string(),
        "authority resource identity must be at most 4096 bytes"
    );

    let nul = AuthorityResourceId::new("a\0b").err().ok_or("NUL must be rejected")?;
    assert_eq!(nul, AuthorityResourceIdError::NulByte);
    assert_eq!(nul.to_string(), "authority resource identity must not contain NUL");
    Ok(())
}

#[test]
fn held_lease_reports_the_lease_and_epoch_of_a_targeted_assignment() -> TestResult {
    let held = HeldLease::Targeted(identity("lease-a", Some(LeaseEpoch::new(3)?))?);
    assert_eq!(held.lease_id(), &LeaseId::new("lease-a"));
    assert_eq!(held.epoch(), Some(LeaseEpoch::new(3)?));

    let unassigned = HeldLease::Targeted(identity("lease-b", None)?);
    assert_eq!(unassigned.epoch(), None);
    Ok(())
}

#[test]
fn held_lease_reports_no_epoch_for_a_provider_resource() -> TestResult {
    let held = HeldLease::Provider {
        lease_id: LeaseId::new("lease-p"),
        provider: ProviderKind::Subprocess,
        resource_id: AuthorityResourceId::new("pid-1")?,
    };
    assert_eq!(held.lease_id(), &LeaseId::new("lease-p"));
    assert_eq!(held.epoch(), None);
    Ok(())
}

#[test]
fn held_leases_compare_and_hash_by_exact_assignment() -> TestResult {
    let first = HeldLease::Targeted(identity("lease-a", Some(LeaseEpoch::FIRST))?);
    let same = HeldLease::Targeted(identity("lease-a", Some(LeaseEpoch::FIRST))?);
    let newer = HeldLease::Targeted(identity("lease-a", Some(LeaseEpoch::new(2)?))?);
    let set = HashSet::from([first.clone(), newer.clone()]);
    assert_eq!(first, same);
    assert_ne!(first, newer);
    assert_eq!(set.len(), 2);
    assert!(set.contains(&same));
    Ok(())
}

#[test]
fn outcomes_compare_and_copy() {
    let frozen = FreezeOutcome::Frozen;
    let copy = frozen;
    assert_eq!(frozen, copy);
    assert_ne!(FreezeOutcome::AlreadyPaused, FreezeOutcome::Unsupported);
    assert_ne!(FreezeOutcome::Stale, FreezeOutcome::Frozen);
    assert_ne!(PurgeOutcome::Purged, PurgeOutcome::Settled);
    assert_ne!(PurgeOutcome::Stale, PurgeOutcome::Purged);
    assert_ne!(ThawOutcome::Thawed, ThawOutcome::NoLongerOwned);
    assert_ne!(ThawOutcome::Stale, ThawOutcome::Thawed);
    assert!(format!("{:?}", ThawOutcome::Stale).contains("Stale"));
}

#[test]
fn task_completion_witness_is_scoped_and_keeps_assignment_hashes_stable() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let foreign = AuthorityTaskScope::default();
    let id = LeaseId::new("task-lease");
    let (resource, completion) = scope.track(id.clone(), "owned-task")?;
    let assignment = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::Subprocess,
        resource_id: resource.clone(),
    };
    let mut pending = HashSet::from([assignment.clone()]);
    assert!(!scope.is_completed(&id, &resource));
    completion.confirm();
    assert!(scope.is_completed(&id, &resource));
    assert!(!foreign.is_completed(&id, &resource));
    assert!(!scope.is_completed(&LeaseId::new("other-lease"), &resource));
    assert!(!scope.is_completed(&id, &AuthorityResourceId::new("owned-task")?));
    assert!(pending.remove(&assignment));
    let (different, _) = scope.track(id, "owned-task")?;
    assert_ne!(resource, different);
    Ok(())
}

#[test]
fn task_tracking_rejects_an_invalid_resource() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let error = scope
        .track(LeaseId::new("lease"), "")
        .err()
        .ok_or("an empty resource must be rejected")?;
    assert_eq!(error, AuthorityResourceIdError::Empty);
    assert!(scope.track(LeaseId::new("lease"), "a\0").is_err());
    Ok(())
}

#[test]
fn task_confirmation_is_idempotent_and_shared_by_clones() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let lease = LeaseId::new("lease");
    let (resource, completion) = scope.track(lease.clone(), "task")?;
    let clone = completion.clone();
    assert_eq!(completion, clone);
    clone.confirm();
    clone.confirm();
    assert!(scope.is_completed(&lease, &resource));
    completion.confirm();
    assert!(scope.is_completed(&lease, &resource));
    Ok(())
}

#[test]
fn task_completions_are_equal_only_to_the_same_handle() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let lease = LeaseId::new("lease");
    let (_, first) = scope.track(lease.clone(), "task")?;
    let (_, second) = scope.track(lease, "task")?;
    assert_ne!(first, second);
    let set = HashSet::from([first.clone(), second]);
    assert_eq!(set.len(), 2);
    assert!(set.contains(&first.clone()));
    Ok(())
}

#[test]
fn an_untracked_resource_is_never_completed() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let lease = LeaseId::new("lease");
    let plain = AuthorityResourceId::new("task")?;
    assert!(!scope.is_completed(&lease, &plain));
    Ok(())
}

#[test]
fn a_tracked_task_is_not_completed_for_a_resource_tracked_separately() -> TestResult {
    let scope = AuthorityTaskScope::default();
    let lease = LeaseId::new("lease");
    let (first, first_completion) = scope.track(lease.clone(), "task")?;
    let (second, _) = scope.track(lease.clone(), "task")?;
    first_completion.confirm();
    assert!(scope.is_completed(&lease, &first));
    assert!(!scope.is_completed(&lease, &second));
    Ok(())
}

#[test]
fn host_errors_describe_each_failure() -> TestResult {
    let failed = AuthorityHostError::Failed("pause failed".into());
    assert_eq!(failed.to_string(), "pause failed");
    assert_eq!(
        AuthorityHostError::CleanupFailed {
            message: "backend down".to_owned()
        }
        .to_string(),
        "backend down"
    );
    assert_eq!(
        AuthorityHostError::ConfirmationMissing.to_string(),
        "exact backend cleanup lacks durable confirmation"
    );
    assert_eq!(
        AuthorityHostError::CleanupReplyMismatch.to_string(),
        "cleanup reply does not confirm the exact assignment and purge intent"
    );
    let ledger = AuthorityHostError::from(trogon_sandbox_protocol::AttemptLedgerError::Locked);
    assert_eq!(ledger.to_string(), "another worker owns the dispatch ledger");
    assert!(matches!(ledger, AuthorityHostError::Ledger(_)));
    Ok(())
}

#[test]
fn oracle_errors_describe_each_failure() -> TestResult {
    assert_eq!(
        OracleError::NoResponders.to_string(),
        "no responders for the authority verb"
    );
    assert_eq!(OracleError::TimedOut.to_string(), "authority request timed out");
    assert_eq!(OracleError::Failed("broker down".into()).to_string(), "broker down");

    let json_error = serde_json::from_str::<u32>("not json")
        .err()
        .ok_or("invalid JSON must fail")?;
    let detail = json_error.to_string();
    assert_eq!(
        OracleError::Encode(json_error).to_string(),
        format!("authority request could not be encoded: {detail}")
    );
    let json_error = serde_json::from_str::<u32>("not json")
        .err()
        .ok_or("invalid JSON must fail")?;
    assert_eq!(
        OracleError::Decode(json_error).to_string(),
        format!("authority answer could not be read: {detail}")
    );

    let request = async_nats::RequestError::new(async_nats::client::RequestErrorKind::TimedOut);
    assert!(
        OracleError::Request(request)
            .to_string()
            .starts_with("authority request failed: ")
    );
    Ok(())
}
