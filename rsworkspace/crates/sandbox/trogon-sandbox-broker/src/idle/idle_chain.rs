use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{IdleAction, IdleStage, LeaseRecord, LeaseStatus, SandboxSpec};

use super::idle_capabilities::IdleCapabilities;
use super::idle_pauser::IdlePauser;

/// The idle stages in force for `spec`: the explicit policy, else the idle timeout as a single
/// release stage, else none.
pub fn idle_stages(spec: &SandboxSpec) -> Vec<IdleStage> {
    match (spec.idle_policy(), spec.idle_timeout_seconds()) {
        (Some(policy), _) => policy.stages().to_vec(),
        (None, Some(after_seconds)) => vec![IdleStage {
            after_seconds,
            action: IdleAction::Release,
        }],
        (None, None) => Vec::new(),
    }
}

/// When the lease last saw activity, never earlier than its creation.
pub fn last_activity(record: &LeaseRecord) -> SystemTime {
    record
        .last_activity_at
        .map_or(record.created_at, |seen| seen.max(record.created_at))
}

/// The idle action due for `record` at `now`, if any, with suspend treated as
/// unavailable. See [`next_idle_action_with`].
pub fn next_idle_action(record: &LeaseRecord, now: SystemTime, can_pause: bool) -> Option<IdleAction> {
    next_idle_action_with(
        record,
        now,
        IdleCapabilities {
            pause: can_pause,
            suspend: false,
        },
    )
}

/// The idle action due for `record` at `now`, if any.
///
/// A stage the provider cannot perform is skipped, so the next stage still
/// fires. A Suspend stage on a provider that cannot suspend degrades to Pause,
/// the nearest milder way to stop the lease costing host resources; when it
/// cannot pause either, the stage is skipped. Pure, so the legacy sweeper and
/// the targeted-attempt reconciler cannot disagree.
pub fn next_idle_action_with(record: &LeaseRecord, now: SystemTime, caps: IdleCapabilities) -> Option<IdleAction> {
    let stages = idle_stages(&record.spec);
    if stages.is_empty() {
        return None;
    }
    let quiet = now.duration_since(last_activity(record)).ok()?;
    let due = |action: IdleAction| {
        stages
            .iter()
            .any(|stage| stage.action == action && quiet > Duration::from_secs(u64::from(stage.after_seconds.get())))
    };
    let released = due(IdleAction::Release);
    let suspend_due = due(IdleAction::Suspend);
    match record.status {
        LeaseStatus::Running => {
            if released {
                Some(IdleAction::Release)
            } else if suspend_due && caps.suspend {
                Some(IdleAction::Suspend)
            } else if caps.pause && (suspend_due || due(IdleAction::Pause)) {
                Some(IdleAction::Pause)
            } else {
                None
            }
        }
        LeaseStatus::Paused if record.auto_paused_at.is_some() => {
            if released {
                Some(IdleAction::Release)
            } else if suspend_due && caps.suspend {
                Some(IdleAction::Suspend)
            } else {
                None
            }
        }
        // A suspended lease is bounded by its suspended budget, not the chain.
        _ => None,
    }
}

/// Whether the record has a Pause stage, so a sweeper only asks the provider
/// about pause support when it matters.
pub fn has_pause_stage(record: &LeaseRecord) -> bool {
    idle_stages(&record.spec)
        .iter()
        .any(|stage| stage.action == IdleAction::Pause)
}

/// Whether the record has a Suspend stage.
pub fn has_suspend_stage(record: &LeaseRecord) -> bool {
    idle_stages(&record.spec)
        .iter()
        .any(|stage| stage.action == IdleAction::Suspend)
}

/// Asks `pauser` only what the record's stages make relevant. A Suspend stage
/// also needs pause support, because it degrades to Pause.
pub async fn idle_capabilities(record: &LeaseRecord, pauser: &dyn IdlePauser) -> IdleCapabilities {
    let wants_suspend = has_suspend_stage(record);
    IdleCapabilities {
        pause: (has_pause_stage(record) || wants_suspend) && pauser.can_pause(record.provider).await,
        suspend: wants_suspend && pauser.can_suspend(record).await,
    }
}
