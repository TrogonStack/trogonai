use std::time::Duration;

use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;

use crate::k8s_ops::ApiServerClock;
use crate::k8s_ops::constants::LABEL_LEASE_DEADLINE;

/// Whether the Job controller has finished `job`. A finished lease Job has no
/// pod left to exec in, whether it completed, failed, or ran out its
/// `activeDeadlineSeconds`.
pub fn job_is_finished(job: &Job) -> bool {
    job.status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                matches!(condition.type_.as_str(), "Complete" | "Failed") && condition.status == "True"
            })
        })
}

/// Whether `job` has lived past its `activeDeadlineSeconds`, measured on the
/// apiserver clock against the apiserver-stamped `creationTimestamp`. The
/// deadline is the lease TTL the Job was launched with, and the Job carries
/// it, so no worker-side record of it is needed. A Job with no deadline or no
/// timestamp never counts as expired here.
pub fn job_outlived_deadline(job: &Job, now: ApiServerClock) -> bool {
    job_outlived_deadline_by(job, now, 0)
}

/// Whether `job` outlived its `activeDeadlineSeconds` by at least `grace`
/// seconds on the apiserver clock.
pub(crate) fn job_outlived_deadline_by(job: &Job, now: ApiServerClock, grace: i64) -> bool {
    let Some(created) = job.metadata.creation_timestamp.as_ref() else {
        return false;
    };
    let Some(deadline) = job.spec.as_ref().and_then(|spec| spec.active_deadline_seconds) else {
        return false;
    };
    now.as_second() - created.0.as_second() >= deadline.saturating_add(grace)
}

/// Whether claimed warm pod `pod` has outlived the deadline its claim stamped
/// in [`LABEL_LEASE_DEADLINE`], on the apiserver clock. A pod with no deadline
/// label, or one that does not parse, never counts as expired here.
pub fn warm_pod_outlived_deadline(pod: &Pod, now: ApiServerClock) -> bool {
    warm_pod_outlived_deadline_by(pod, now, 0)
}

/// Whether `pod` outlived its lease deadline by at least `grace` seconds.
pub(crate) fn warm_pod_outlived_deadline_by(pod: &Pod, now: ApiServerClock, grace: i64) -> bool {
    pod.metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(LABEL_LEASE_DEADLINE))
        .and_then(|value| value.parse::<i64>().ok())
        .is_some_and(|deadline| now.as_second() >= deadline.saturating_add(grace))
}

/// The Job's `activeDeadlineSeconds` for a sandbox `timeout`. A zero timeout
/// is a lease with no deadline, so the Job gets none and the sweeper leaves
/// it to release or the broker's idle reaper.
pub(crate) fn job_deadline_seconds(timeout: Duration) -> Option<i64> {
    (!timeout.is_zero()).then(|| i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX))
}
