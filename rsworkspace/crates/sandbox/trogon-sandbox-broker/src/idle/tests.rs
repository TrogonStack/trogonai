use std::collections::BTreeMap;

use std::num::NonZeroU32;

use trogon_sandbox_contract::{
    IdlePolicy, IdleStage, LeaseId, OciImageRef, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId,
    WorkspaceSource,
};

use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{IdlePauseRetry, LeaseRecord, LeaseStatus, ProviderKind};

use super::*;

const T0: u64 = 1_000_000;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// A minimal valid spec with the given idle configuration.
pub(crate) fn spec(
    cpu_seconds: u32,
    idle_timeout_seconds: Option<u32>,
    stages: Option<&[(u32, IdleAction)]>,
) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    let idle_policy = match stages {
        Some(stages) if !stages.is_empty() => {
            let mut built = Vec::new();
            for &(after_seconds, action) in stages {
                built.push(IdleStage {
                    after_seconds: NonZeroU32::new(after_seconds).ok_or("zero idle stage")?,
                    action,
                });
            }
            Some(IdlePolicy::new(built)?)
        }
        _ => None,
    };
    let idle_timeout_seconds = match idle_timeout_seconds {
        Some(seconds) => Some(NonZeroU32::new(seconds).ok_or("zero idle timeout")?),
        None => None,
    };
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(cpu_seconds.max(1)).ok_or("zero cpu")?,
        memory_mib: NonZeroU32::MIN,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds,
        no_ttl: Default::default(),
        idle_policy,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?)
}

pub(crate) fn record(stages: &[(u32, IdleAction)]) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    let spec = spec(1, None, Some(stages))?;
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new("l"),
        tenant: TenantId::new("t")?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: at(T0),
        deadline: at(T0 + 100_000),
        spec,
        sandbox_id: "s".into(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: None,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

/// Applies the evaluator's verdict to the record the way the broker does.
fn apply(record: &mut LeaseRecord, action: IdleAction, now: SystemTime) {
    match action {
        IdleAction::Pause => {
            record.status = LeaseStatus::Paused;
            record.auto_paused_at = Some(now);
        }
        IdleAction::Suspend => record.status = LeaseStatus::Suspended,
        IdleAction::Release => record.status = LeaseStatus::Released,
    }
}

const CHAIN: [(u32, IdleAction); 2] = [(60, IdleAction::Pause), (600, IdleAction::Release)];

#[test]
fn a_failing_idle_pause_backs_off_exponentially_up_to_the_cap() {
    let mut retry = None;
    let mut now = at(T0);
    assert!(IdlePauseBackoff::due(retry.as_ref(), now));

    let mut delays = Vec::new();
    for _ in 0..8 {
        retry = Some(IdlePauseBackoff::after_failure(retry.as_ref(), now));
        let mut wait = 0;
        while !IdlePauseBackoff::due(retry.as_ref(), now + Duration::from_secs(wait)) {
            wait += 1;
        }
        delays.push(wait);
        now += Duration::from_secs(wait);
    }
    assert_eq!(delays, vec![30, 60, 120, 240, 480, 900, 900, 900]);
}

#[test]
fn a_cleared_backoff_starts_over() {
    let first = IdlePauseBackoff::after_failure(None, at(T0));
    let second = IdlePauseBackoff::after_failure(Some(&first), at(T0 + 30));
    assert!(!IdlePauseBackoff::due(Some(&second), at(T0 + 31)));
    // A successful pause clears the record's backoff.
    let after_success = IdlePauseBackoff::after_failure(None, at(T0 + 91));
    assert_eq!(after_success.retry_at, at(T0 + 121));
}

#[test]
fn a_long_stale_failure_no_longer_counts() {
    let old = IdlePauseRetry {
        failures: 5,
        retry_at: at(T0),
    };
    let next = IdlePauseBackoff::after_failure(Some(&old), at(T0) + IdlePauseBackoff::MAX);
    assert_eq!(next.failures, 1);
    let recent = IdlePauseBackoff::after_failure(Some(&old), at(T0 + 1));
    assert_eq!(recent.failures, 6);
}

#[test]
fn legacy_idle_timeout_is_a_single_release_stage() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&[])?;
    r.spec = spec(1, Some(30), None)?;
    assert_eq!(next_idle_action(&r, at(T0 + 30), true), None);
    assert_eq!(next_idle_action(&r, at(T0 + 31), false), Some(IdleAction::Release));
    r.status = LeaseStatus::Paused;
    assert_eq!(next_idle_action(&r, at(T0 + 9999), true), None);
    Ok(())
}

#[test]
fn no_idle_configuration_never_fires() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&[])?;
    r.spec = spec(1, None, None)?;
    assert_eq!(next_idle_action(&r, at(T0 + 9999), true), None);
    Ok(())
}

#[test]
fn pause_then_release_each_fire_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&CHAIN)?;
    assert_eq!(next_idle_action(&r, at(T0 + 60), true), None);
    let now = at(T0 + 61);
    assert_eq!(next_idle_action(&r, now, true), Some(IdleAction::Pause));
    apply(&mut r, IdleAction::Pause, now);
    // The pause stage does not fire again, and release is not due yet.
    assert_eq!(next_idle_action(&r, at(T0 + 599), true), None);
    assert_eq!(next_idle_action(&r, at(T0 + 601), true), Some(IdleAction::Release));
    Ok(())
}

#[test]
fn a_stage_never_fires_twice_over_a_whole_timeline() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&CHAIN)?;
    let mut fired = Vec::new();
    for tick in 0..2000u64 {
        let now = at(T0 + tick);
        if let Some(action) = next_idle_action(&r, now, true) {
            fired.push(action);
            apply(&mut r, action, now);
        }
    }
    assert_eq!(fired, vec![IdleAction::Pause, IdleAction::Release]);
    Ok(())
}

#[test]
fn severity_never_decreases_for_any_due_time() -> Result<(), Box<dyn std::error::Error>> {
    for quiet in 0..2000u64 {
        let mut r = record(&CHAIN)?;
        let mut last = None;
        for step in [quiet, quiet + 1, quiet + 700] {
            let now = at(T0 + step);
            if let Some(action) = next_idle_action(&r, now, true) {
                assert!(last.is_none_or(|prev| action > prev), "{last:?} then {action:?}");
                last = Some(action);
                apply(&mut r, action, now);
            }
        }
    }
    Ok(())
}

#[test]
fn activity_restarts_the_chain() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&CHAIN)?;
    let now = at(T0 + 61);
    let action = next_idle_action(&r, now, true).ok_or("no idle action")?;
    apply(&mut r, action, now);
    // Resume clears the marker and records fresh activity.
    r.status = LeaseStatus::Running;
    r.auto_paused_at = None;
    r.last_activity_at = Some(at(T0 + 100));
    assert_eq!(next_idle_action(&r, at(T0 + 160), true), None);
    assert_eq!(next_idle_action(&r, at(T0 + 161), true), Some(IdleAction::Pause));
    Ok(())
}
#[test]
fn pause_is_skipped_when_unsupported_and_release_still_fires() -> Result<(), Box<dyn std::error::Error>> {
    let r = record(&CHAIN)?;
    assert_eq!(next_idle_action(&r, at(T0 + 61), false), None);
    assert_eq!(next_idle_action(&r, at(T0 + 599), false), None);
    assert_eq!(next_idle_action(&r, at(T0 + 601), false), Some(IdleAction::Release));
    Ok(())
}

#[test]
fn a_late_sweeper_skips_pause_and_releases_directly() -> Result<(), Box<dyn std::error::Error>> {
    let r = record(&CHAIN)?;
    assert_eq!(next_idle_action(&r, at(T0 + 5000), true), Some(IdleAction::Release));
    Ok(())
}

#[test]
fn a_lease_paused_by_request_is_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&CHAIN)?;
    r.status = LeaseStatus::Paused;
    assert_eq!(next_idle_action(&r, at(T0 + 5000), true), None);
    Ok(())
}

#[test]
fn other_states_never_fire() -> Result<(), Box<dyn std::error::Error>> {
    for status in [
        LeaseStatus::Pending,
        LeaseStatus::Suspending,
        LeaseStatus::Suspended,
        LeaseStatus::Waking,
        LeaseStatus::Cancelling,
        LeaseStatus::Released,
        LeaseStatus::Failed,
    ] {
        let mut r = record(&CHAIN)?;
        r.status = status;
        assert_eq!(next_idle_action(&r, at(T0 + 5000), true), None);
    }
    Ok(())
}

#[test]
fn a_pause_only_policy_never_releases() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&[(60, IdleAction::Pause)])?;
    let now = at(T0 + 61);
    assert_eq!(next_idle_action(&r, now, true), Some(IdleAction::Pause));
    apply(&mut r, IdleAction::Pause, now);
    assert_eq!(next_idle_action(&r, at(T0 + 100_000), true), None);
    Ok(())
}

const BOTH: IdleCapabilities = IdleCapabilities {
    pause: true,
    suspend: true,
};

#[test]
fn suspend_fires_between_pause_and_release() -> Result<(), Box<dyn std::error::Error>> {
    let mut r = record(&[
        (60, IdleAction::Pause),
        (300, IdleAction::Suspend),
        (600, IdleAction::Release),
    ])?;
    assert_eq!(next_idle_action_with(&r, at(T0 + 61), BOTH), Some(IdleAction::Pause));
    apply(&mut r, IdleAction::Pause, at(T0 + 61));
    assert_eq!(next_idle_action_with(&r, at(T0 + 299), BOTH), None);
    assert_eq!(next_idle_action_with(&r, at(T0 + 301), BOTH), Some(IdleAction::Suspend));
    apply(&mut r, IdleAction::Suspend, at(T0 + 301));
    assert_eq!(next_idle_action_with(&r, at(T0 + 9999), BOTH), None);
    Ok(())
}

#[test]
fn suspend_degrades_to_pause_when_the_worker_cannot_suspend() -> Result<(), Box<dyn std::error::Error>> {
    let r = record(&[(300, IdleAction::Suspend)])?;
    let pause_only = IdleCapabilities {
        pause: true,
        suspend: false,
    };
    assert_eq!(
        next_idle_action_with(&r, at(T0 + 301), pause_only),
        Some(IdleAction::Pause)
    );
    Ok(())
}

#[test]
fn suspend_is_skipped_when_nothing_milder_is_supported() -> Result<(), Box<dyn std::error::Error>> {
    let r = record(&[(300, IdleAction::Suspend), (900, IdleAction::Release)])?;
    let none = IdleCapabilities::default();
    assert_eq!(next_idle_action_with(&r, at(T0 + 301), none), None);
    assert_eq!(next_idle_action_with(&r, at(T0 + 901), none), Some(IdleAction::Release));
    Ok(())
}

#[test]
fn a_late_sweeper_suspends_directly() -> Result<(), Box<dyn std::error::Error>> {
    let r = record(&[(60, IdleAction::Pause), (300, IdleAction::Suspend)])?;
    assert_eq!(
        next_idle_action_with(&r, at(T0 + 5000), BOTH),
        Some(IdleAction::Suspend)
    );
    Ok(())
}
