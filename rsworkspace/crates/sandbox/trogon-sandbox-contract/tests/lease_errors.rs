use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, Digest, LeaseError, LeaseId, OperationId, ProviderKind, SnapshotTag,
    SnapshotTagName, SuspendRequestLevel, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Case = (LeaseError, Value, &'static str);

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("acme")?)
}

fn digest() -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(DIGEST)?)
}

fn operation() -> Result<OperationId, Box<dyn std::error::Error>> {
    Ok(OperationId::new("op-1")?)
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn text(value: &str) -> String {
    value.to_owned()
}

fn cases() -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    Ok(vec![
        (
            LeaseError::QuotaExceeded { tenant: tenant()? },
            json!({ "kind": "quota_exceeded", "tenant": "acme" }),
            "quota exceeded for tenant acme",
        ),
        (
            LeaseError::NoProviderAvailable,
            json!({ "kind": "no_provider_available" }),
            "no provider available",
        ),
        (
            LeaseError::InvalidSpec { reason: text("bad") },
            json!({ "kind": "invalid_spec", "reason": "bad" }),
            "invalid spec: bad",
        ),
        (
            LeaseError::InvalidNetworkPolicy { reason: text("bad") },
            json!({ "kind": "invalid_network_policy", "reason": "bad" }),
            "invalid network policy: bad",
        ),
        (
            LeaseError::Backend { message: text("down") },
            json!({ "kind": "backend", "message": "down" }),
            "backend error: down",
        ),
        (
            LeaseError::Unauthorized { reason: text("no") },
            json!({ "kind": "unauthorized", "reason": "no" }),
            "unauthorized: no",
        ),
        (
            LeaseError::NoWorkerSatisfiesCapabilities {
                requirements: CapabilityRequirements {
                    needs_snapshot: CapabilityNeed::Required,
                    ..CapabilityRequirements::default()
                },
            },
            json!({
                "kind": "no_worker_satisfies_capabilities",
                "requirements": serde_json::to_value(CapabilityRequirements {
                    needs_snapshot: CapabilityNeed::Required,
                    ..CapabilityRequirements::default()
                })?,
            }),
            "no worker satisfies required capabilities",
        ),
        (
            LeaseError::NoWorkerCanRestoreSnapshot { digest: digest()? },
            json!({ "kind": "no_worker_can_restore_snapshot", "digest": DIGEST }),
            "no worker can restore snapshot sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
        (
            LeaseError::TooManyActiveLeases {
                tenant: tenant()?,
                cap: 5,
            },
            json!({ "kind": "too_many_active_leases", "tenant": "acme", "cap": 5 }),
            "too many active leases for tenant acme (cap 5)",
        ),
        (
            LeaseError::TooManyTaggedLeases {
                tenant: tenant()?,
                tag_key: text("user"),
                tag_value: text("ada"),
                cap: 2,
            },
            json!({
                "kind": "too_many_tagged_leases",
                "tenant": "acme",
                "tag_key": "user",
                "tag_value": "ada",
                "cap": 2,
            }),
            "too many active leases for user=ada in tenant acme (cap 2)",
        ),
        (
            LeaseError::SnapshotQuotaExceeded { tenant: tenant()? },
            json!({ "kind": "snapshot_quota_exceeded", "tenant": "acme" }),
            "snapshot quota exceeded for tenant acme",
        ),
        (
            LeaseError::SnapshotTagNotFound {
                tag: SnapshotTag {
                    tenant: tenant()?,
                    name: SnapshotTagName::new("golden")?,
                },
            },
            json!({ "kind": "snapshot_tag_not_found", "tag": { "tenant": "acme", "name": "golden" } }),
            "snapshot tag not found: acme:golden",
        ),
        (
            LeaseError::SnapshotUnsupported {
                provider: ProviderKind::Fly,
                reason: text("no disk"),
            },
            json!({ "kind": "snapshot_unsupported", "provider": "fly", "reason": "no disk" }),
            "snapshot unsupported on Fly: no disk",
        ),
        (
            LeaseError::SnapshotSignatureInvalid { digest: digest()? },
            json!({ "kind": "snapshot_signature_invalid", "digest": DIGEST }),
            "snapshot signature invalid for digest sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
        (
            LeaseError::PrewarmQuotaExceeded { tenant: tenant()? },
            json!({ "kind": "prewarm_quota_exceeded", "tenant": "acme" }),
            "prewarm quota exceeded for tenant acme",
        ),
        (
            LeaseError::ImageNotReady {
                image: text("img"),
                reason: text("pulling"),
            },
            json!({ "kind": "image_not_ready", "image": "img", "reason": "pulling" }),
            "image not ready: img (pulling)",
        ),
        (
            LeaseError::SourceLeaseNotFound { lease_id: lease() },
            json!({ "kind": "source_lease_not_found", "lease_id": "lease-1" }),
            "source lease not found: lease-1",
        ),
        (
            LeaseError::SourceLeaseNotRunning { lease_id: lease() },
            json!({ "kind": "source_lease_not_running", "lease_id": "lease-1" }),
            "source lease is not running: lease-1",
        ),
        (
            LeaseError::CheckpointRunningUnsupported {
                provider: ProviderKind::Modal,
            },
            json!({ "kind": "checkpoint_running_unsupported", "provider": "modal" }),
            "checkpoint_running unsupported on Modal",
        ),
        (
            LeaseError::SnapshotsDisabled { lease_id: lease() },
            json!({ "kind": "snapshots_disabled", "lease_id": "lease-1" }),
            "snapshots are disabled for lease lease-1",
        ),
        (
            LeaseError::CheckpointRunningFailed { reason: text("io") },
            json!({ "kind": "checkpoint_running_failed", "reason": "io" }),
            "checkpoint_running failed: io",
        ),
        (
            LeaseError::LeaseNotFound { lease_id: lease() },
            json!({ "kind": "lease_not_found", "lease_id": "lease-1" }),
            "lease not found: lease-1",
        ),
        (
            LeaseError::LeaseNotRunning { lease_id: lease() },
            json!({ "kind": "lease_not_running", "lease_id": "lease-1" }),
            "lease is not running: lease-1",
        ),
        (
            LeaseError::InvalidTags { reason: text("six") },
            json!({ "kind": "invalid_tags", "reason": "six" }),
            "invalid tags: six",
        ),
        (
            LeaseError::WouldExceedMaxLifetime { max: 3600 },
            json!({ "kind": "would_exceed_max_lifetime", "max": 3600 }),
            "extension would exceed max lease lifetime of 3600s",
        ),
        (
            LeaseError::ProviderLaunchFailed {
                provider: ProviderKind::Daytona,
                reason: text("oom"),
            },
            json!({ "kind": "provider_launch_failed", "provider": "daytona", "reason": "oom" }),
            "Daytona worker could not launch the sandbox: oom",
        ),
        (
            LeaseError::ProviderRejectedDispatch {
                provider: ProviderKind::E2b,
                reason: text("busy"),
            },
            json!({ "kind": "provider_rejected_dispatch", "provider": "e2b", "reason": "busy" }),
            "E2b worker rejected the dispatch: busy",
        ),
        (
            LeaseError::FileTooLargeForTransport {
                path: text("/a"),
                bytes: 10,
                max_payload_bytes: 4,
            },
            json!({ "kind": "file_too_large_for_transport", "path": "/a", "bytes": 10, "max_payload_bytes": 4 }),
            "/a is 10 bytes, too large to return in one 4-byte bus message",
        ),
        (
            LeaseError::CommandOutputTooLargeForTransport {
                exit_code: 1,
                stdout_bytes: 8,
                stderr_bytes: 2,
                max_payload_bytes: 4,
            },
            json!({
                "kind": "command_output_too_large_for_transport",
                "exit_code": 1,
                "stdout_bytes": 8,
                "stderr_bytes": 2,
                "max_payload_bytes": 4,
            }),
            "command exited 1 with 8 bytes of stdout and 2 of stderr, too large to return in one 4-byte bus message; stream the output instead",
        ),
        (
            LeaseError::FilesTooLargeForTransport {
                content_bytes: 9,
                max_payload_bytes: 4,
            },
            json!({ "kind": "files_too_large_for_transport", "content_bytes": 9, "max_payload_bytes": 4 }),
            "9 bytes of file content are too large to send in one 4-byte bus message",
        ),
        (
            LeaseError::NameConflict { name: text("dev") },
            json!({ "kind": "name_conflict", "name": "dev" }),
            "named lease 'dev' is being created by another request; retry",
        ),
        (
            LeaseError::NativePassthroughNotPermitted { tenant: tenant()? },
            json!({ "kind": "native_passthrough_not_permitted", "tenant": "acme" }),
            "vendor passthrough is not permitted for tenant acme",
        ),
        (
            LeaseError::InvalidNativeOptions {
                provider: ProviderKind::Vercel,
                reason: text("bad key"),
            },
            json!({ "kind": "invalid_native_options", "provider": "vercel", "reason": "bad key" }),
            "invalid native options for Vercel: bad key",
        ),
        (
            LeaseError::NoWorkerInRegion {
                regions: vec![text("eu"), text("us")],
            },
            json!({ "kind": "no_worker_in_region", "regions": ["eu", "us"] }),
            "no worker can place the sandbox in any of the regions: eu, us",
        ),
        (
            LeaseError::OperationConflict {
                operation_id: operation()?,
                reason: text("reused"),
            },
            json!({ "kind": "operation_conflict", "operation_id": "op-1", "reason": "reused" }),
            "operation op-1 conflicts: reused",
        ),
        (
            LeaseError::OperationNotFound {
                operation_id: operation()?,
            },
            json!({ "kind": "operation_not_found", "operation_id": "op-1" }),
            "operation op-1 not found",
        ),
        (
            LeaseError::OperationCanceled {
                operation_id: operation()?,
            },
            json!({ "kind": "operation_canceled", "operation_id": "op-1" }),
            "operation op-1 was canceled",
        ),
        (
            LeaseError::SuspendUnsupported {
                lease_id: lease(),
                requested: SuspendRequestLevel::Memory,
            },
            json!({ "kind": "suspend_unsupported", "lease_id": "lease-1", "requested": "memory" }),
            "lease lease-1 cannot suspend to level memory: the provider does not offer it",
        ),
        (
            LeaseError::LeaseWaking {
                lease_id: lease(),
                retry_after_ms: 500,
            },
            json!({ "kind": "lease_waking", "lease_id": "lease-1", "retry_after_ms": 500 }),
            "lease lease-1 is waking; retry in 500ms",
        ),
        (
            LeaseError::SuspendArtifactLost { lease_id: lease() },
            json!({ "kind": "suspend_artifact_lost", "lease_id": "lease-1" }),
            "the suspend artifact of lease lease-1 is lost",
        ),
        (
            LeaseError::EntityUnavailable { reason: text("store") },
            json!({ "kind": "entity_unavailable", "reason": "store" }),
            "sandbox entity unavailable: store",
        ),
        (
            LeaseError::EntityNotEnabled,
            json!({ "kind": "entity_not_enabled" }),
            "sandbox entities are not enabled on this broker",
        ),
    ])
}

#[test]
fn every_lease_error_is_tagged_by_kind_and_keeps_its_message() -> TestResult {
    let cases = cases()?;
    assert_eq!(cases.len(), 42);
    for (error, wire, message) in cases {
        assert_eq!(error.to_string(), message);
        assert_eq!(serde_json::to_value(&error)?, wire, "{message}");
        assert_eq!(serde_json::from_value::<LeaseError>(wire)?, error, "{message}");
    }
    Ok(())
}
