use super::constants::{FREEZE_SETTLE_BACKOFF_BASE, FREEZE_SETTLE_BACKOFF_MAX};
use super::region_error::RegionError;
use super::spec_validation_error::SpecValidationError;
use super::*;

/// `Nats-Msg-Id` of a lease's `lease_released` event. A lease is released
/// once, so every publish of its event, a retry or a reconciler's re-announce
/// included, carries the same id and dedupes as one message.
pub fn lease_released_msg_id(lease_id: &LeaseId) -> String {
    format!("lease_released.{}", lease_id.as_str())
}

/// Wait before the next settle attempt after `attempts` refusals: doubling
/// from the base, capped.
pub(super) fn freeze_settle_backoff(attempts: u32) -> Duration {
    let doublings = attempts.saturating_sub(1).min(16);
    FREEZE_SETTLE_BACKOFF_BASE
        .saturating_mul(1 << doublings)
        .min(FREEZE_SETTLE_BACKOFF_MAX)
}

/// Wraps any displayable failure as a backend lease error.
pub(super) fn backend(error: impl std::fmt::Display) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

/// The `lease_released` event for a released lease.
pub(super) fn announcement(record: &LeaseRecord, reason: LeaseReleasedReason) -> LeaseReleased {
    LeaseReleased {
        lease_id: record.lease_id.clone(),
        tenant: record.tenant.clone(),
        reason,
        snapshot_out: record.spec.snapshot_out().cloned(),
        worker_id: record.worker_id.clone(),
        correlation_id: record.correlation_id.clone(),
        restored_from: record.restored_from.clone(),
        released_at: Some(SystemTime::now()),
    }
}

/// True when a provider dispatch failure means "no substrate workers are
/// deployed" rather than a genuine timeout or transport error. Relies on
/// [`crate::nats_bus::NatsBus::request_with_timeout`] preserving the original
/// `async_nats::RequestError` through `anyhow`'s blanket conversion.
pub(super) fn is_no_responders(err: &anyhow::Error) -> bool {
    err.downcast_ref::<async_nats::RequestError>()
        .is_some_and(|e| e.kind() == async_nats::RequestErrorKind::NoResponders)
}

/// How long the broker waits on a worker running a foreground command. The
/// transport's default request timeout is shorter than most command timeouts,
/// so the wait follows the command's own timeout, capped at the time the lease
/// has left, since the worker tears the sandbox down at the lease deadline.
pub(super) fn foreground_run_command_timeout(
    timeout_seconds: u32,
    lease_deadline: SystemTime,
    now: SystemTime,
) -> Duration {
    let command = Duration::from_secs(u64::from(timeout_seconds.max(1)));
    let lease_left = lease_deadline.duration_since(now).unwrap_or_default();
    command.min(lease_left) + PROVIDER_RUN_COMMAND_GRACE
}

pub(super) fn retired_launch_reply() -> RunCommandResponse {
    RunCommandResponse::Denied(LeaseError::InvalidSpec {
        reason: "launch_key already ran on this lease and its reply is no longer kept".to_string(),
    })
}

pub(super) fn lost_launch_reply() -> RunCommandResponse {
    RunCommandResponse::Denied(LeaseError::Backend {
        message: "the first launch with this launch_key was lost before it replied; \
                  its outcome is unknown"
            .to_string(),
    })
}

pub(super) fn unkept_launch_reply() -> RunCommandResponse {
    RunCommandResponse::Denied(LeaseError::Backend {
        message: "the first launch with this launch_key already got a reply, \
                  but the reply could not be kept for replay; it is not run again"
            .to_string(),
    })
}

/// How long the broker waits on a worker starting a detached command, capped
/// at the time the lease has left like [`foreground_run_command_timeout`].
pub(super) fn detached_start_timeout(lease_deadline: SystemTime, now: SystemTime) -> Duration {
    let lease_left = lease_deadline.duration_since(now).unwrap_or_default();
    PROVIDER_DETACHED_START_TIMEOUT.min(lease_left + PROVIDER_RUN_COMMAND_GRACE)
}

pub(super) fn is_request_timeout(err: &anyhow::Error) -> bool {
    err.downcast_ref::<async_nats::RequestError>()
        .is_some_and(|e| e.kind() == async_nats::RequestErrorKind::TimedOut)
}

pub(super) fn lease_summary(record: &LeaseRecord) -> LeaseSummary {
    LeaseSummary {
        lease_id: record.lease_id.clone(),
        tenant: record.tenant.clone(),
        created_at: record.created_at,
        expires_at: record.deadline,
        status: record.status,
        provider: record.provider,
        tags: record.tags.clone(),
        last_activity_at: record.last_activity_at,
    }
}

/// True when at least `secs` seconds passed between `since` and `now`.
pub(super) fn elapsed_at_least(since: SystemTime, now: SystemTime, secs: u32) -> bool {
    now.duration_since(since)
        .is_ok_and(|elapsed| elapsed >= Duration::from_secs(u64::from(secs)))
}

pub(super) fn validate_spec(req: &LeaseRequest, max_lifetime_seconds: u32) -> Result<(), SpecValidationError> {
    if req.ttl_seconds == 0 {
        return Err(SpecValidationError::ZeroTtl);
    }
    if let Some(idle) = req.spec.idle_timeout_seconds()
        && idle.get() > max_lifetime_seconds
    {
        return Err(SpecValidationError::IdleTimeoutTooLong { max_lifetime_seconds });
    }
    Ok(())
}

/// Spec fields the substrate provider cannot honor.
///
/// These are refused here rather than inside the worker: a worker-side `Provider::lease`
/// error reaches the tenant as `LeaseError::Backend { message: "provider reply decode
/// failed: …" }`, which is indistinguishable from a backend outage. Silently dropping them
/// is worse still, since the tenant would believe they got an isolation or mount guarantee
/// they do not have.
pub(super) fn substrate_unsupported_spec_reason(spec: &trogon_sandbox_contract::SandboxSpec) -> Option<String> {
    if !spec.mounts().is_empty() {
        return Some(
            "remote mounts are not supported on the substrate provider: the actor sandbox has no FUSE".to_string(),
        );
    }
    if !spec.drives().is_empty() {
        return Some(
            "persistent drives are not supported on the substrate provider: actor volumes do not outlive the actor"
                .to_string(),
        );
    }
    if spec.privileged().is_granted() {
        return Some(
            "privileged: true is not supported on the substrate provider: it has no capability plumbing".to_string(),
        );
    }
    if spec.user().is_some() {
        return Some(
            "user overrides are not supported on the substrate provider: the container user is frozen into the actor template"
                .to_string(),
        );
    }
    if spec.memory_mib().get() > crate::constants::SUBSTRATE_MAX_MEMORY_MIB {
        return Some(format!(
            "memory_mib {} exceeds the largest substrate size class ({} MiB)",
            spec.memory_mib(),
            crate::constants::SUBSTRATE_MAX_MEMORY_MIB
        ));
    }
    None
}

/// The regions to try in order: the preferred one, then the fallbacks, with
/// repeats dropped.
pub(super) fn region_order(req: &LeaseRequest) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for region in req.region.iter().chain(req.fallback_regions.iter()) {
        if !order.contains(region) {
            order.push(region.clone());
        }
    }
    order
}

pub(super) fn validate_regions(req: &LeaseRequest) -> Result<(), RegionError> {
    if req.region.is_none() && !req.fallback_regions.is_empty() {
        return Err(RegionError::FallbackWithoutRegion);
    }
    if req
        .region
        .iter()
        .chain(req.fallback_regions.iter())
        .any(|region| region.trim().is_empty())
    {
        return Err(RegionError::EmptyName);
    }
    Ok(())
}

/// Keeps only the entry for `provider` out of a `native` object so a worker
/// never sees options meant for another vendor.
pub(super) fn native_for_provider(native: &serde_json::Value, provider: ProviderKind) -> Option<serde_json::Value> {
    let token = subjects::provider_token(provider);
    let entry = native.as_object()?.get(token)?;
    let mut only = serde_json::Map::new();
    only.insert(token.to_owned(), entry.clone());
    Some(serde_json::Value::Object(only))
}

pub(super) fn build_dispatch_payload(
    req: &LeaseRequest,
    lease_id: &LeaseId,
    correlation_id: &CorrelationId,
    resolved_drives: &[trogon_sandbox_contract::DriveRecord],
) -> Result<Bytes, DispatchPayloadError> {
    let envelope = trogon_sandbox_contract::ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: req.clone(),
        correlation_id: Some(correlation_id.clone()),
        resolved_drives: resolved_drives.to_vec(),
        target: None,
    };
    Ok(Bytes::from(serde_json::to_vec(&envelope)?))
}

pub(super) fn decode_dispatch_reply(provider: ProviderKind, reply: &[u8]) -> Result<CoreProviderLease, LeaseError> {
    match serde_json::from_slice::<ProviderDispatchReply>(reply) {
        Ok(ProviderDispatchReply::Granted(lease)) => Ok(lease),
        Ok(ProviderDispatchReply::LaunchFailed { provider_error }) => Err(LeaseError::ProviderLaunchFailed {
            provider,
            reason: provider_error,
        }),
        Ok(ProviderDispatchReply::DispatchRejected { decode_error }) => Err(LeaseError::ProviderRejectedDispatch {
            provider,
            reason: decode_error,
        }),
        Ok(ProviderDispatchReply::NativeRejected { native_error }) => Err(LeaseError::InvalidNativeOptions {
            provider,
            reason: native_error,
        }),
        Err(e) => Err(LeaseError::Backend {
            message: format!("provider reply decode failed: {e}"),
        }),
    }
}
