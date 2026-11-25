use serde::{Deserialize, Serialize};

use crate::{
    CapabilityRequirements, Digest, LeaseId, LeaseName, OperationId, ProviderKind, SnapshotTag, SuspendRequestLevel,
    TenantId,
};

/// Why a lease request or lease operation was refused, tagged by `kind`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LeaseError {
    #[error("quota exceeded for tenant {tenant}")]
    QuotaExceeded { tenant: TenantId },
    #[error("no provider available")]
    NoProviderAvailable,
    #[error("invalid spec: {reason}")]
    InvalidSpec { reason: String },
    #[error("invalid network policy: {reason}")]
    InvalidNetworkPolicy { reason: String },
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("unauthorized: {reason}")]
    Unauthorized { reason: String },
    #[error("no worker satisfies required capabilities")]
    NoWorkerSatisfiesCapabilities { requirements: CapabilityRequirements },
    #[error("no worker can restore snapshot {digest}")]
    NoWorkerCanRestoreSnapshot { digest: Digest },
    #[error("too many active leases for tenant {tenant} (cap {cap})")]
    TooManyActiveLeases { tenant: TenantId, cap: u32 },
    /// The tenant already holds its per-tag cap of live leases for this tag
    /// value (for example one user's share of the tenant).
    #[error("too many active leases for {tag_key}={tag_value} in tenant {tenant} (cap {cap})")]
    TooManyTaggedLeases {
        tenant: TenantId,
        tag_key: String,
        tag_value: String,
        cap: u32,
    },
    #[error("snapshot quota exceeded for tenant {tenant}")]
    SnapshotQuotaExceeded { tenant: TenantId },
    #[error("snapshot tag not found: {}:{}", .tag.tenant, .tag.name)]
    SnapshotTagNotFound { tag: SnapshotTag },
    #[error("snapshot unsupported on {provider:?}: {reason}")]
    SnapshotUnsupported { provider: ProviderKind, reason: String },
    #[error("snapshot signature invalid for digest {digest}")]
    SnapshotSignatureInvalid { digest: Digest },
    #[error("prewarm quota exceeded for tenant {tenant}")]
    PrewarmQuotaExceeded { tenant: TenantId },
    #[error("image not ready: {image} ({reason})")]
    ImageNotReady { image: String, reason: String },
    #[error("source lease not found: {lease_id}")]
    SourceLeaseNotFound { lease_id: LeaseId },
    #[error("source lease is not running: {lease_id}")]
    SourceLeaseNotRunning { lease_id: LeaseId },
    #[error("checkpoint_running unsupported on {provider:?}")]
    CheckpointRunningUnsupported { provider: ProviderKind },
    /// The lease was created with `retention.snapshots` off, so no disk
    /// capture of it is allowed.
    #[error("snapshots are disabled for lease {lease_id}")]
    SnapshotsDisabled { lease_id: LeaseId },
    #[error("checkpoint_running failed: {reason}")]
    CheckpointRunningFailed { reason: String },
    #[error("lease not found: {lease_id}")]
    LeaseNotFound { lease_id: LeaseId },
    #[error("lease is not running: {lease_id}")]
    LeaseNotRunning { lease_id: LeaseId },
    #[error("invalid tags: {reason}")]
    InvalidTags { reason: String },
    #[error("extension would exceed max lease lifetime of {max}s")]
    WouldExceedMaxLifetime { max: u32 },
    #[error("{provider:?} worker could not launch the sandbox: {reason}")]
    ProviderLaunchFailed { provider: ProviderKind, reason: String },
    #[error("{provider:?} worker rejected the dispatch: {reason}")]
    ProviderRejectedDispatch { provider: ProviderKind, reason: String },
    #[error("{path} is {bytes} bytes, too large to return in one {max_payload_bytes}-byte bus message")]
    FileTooLargeForTransport {
        path: String,
        bytes: u64,
        max_payload_bytes: u64,
    },
    #[error(
        "command exited {exit_code} with {stdout_bytes} bytes of stdout and {stderr_bytes} of stderr, too large to return in one {max_payload_bytes}-byte bus message; stream the output instead"
    )]
    CommandOutputTooLargeForTransport {
        exit_code: i32,
        stdout_bytes: u64,
        stderr_bytes: u64,
        max_payload_bytes: u64,
    },
    #[error("{content_bytes} bytes of file content are too large to send in one {max_payload_bytes}-byte bus message")]
    FilesTooLargeForTransport { content_bytes: u64, max_payload_bytes: u64 },
    /// Another request is still provisioning a lease under this name, so the
    /// name cannot be resolved yet. The caller retries with the same
    /// idempotency key.
    #[error("named lease '{name}' is being created by another request; retry")]
    NameConflict { name: LeaseName },
    /// The tenant policy does not allow vendor passthrough (`native`).
    #[error("vendor passthrough is not permitted for tenant {tenant}")]
    NativePassthroughNotPermitted { tenant: TenantId },
    /// The worker rejected `native`: an unknown key, a bad value, or options
    /// keyed to a different provider.
    #[error("invalid native options for {provider:?}: {reason}")]
    InvalidNativeOptions { provider: ProviderKind, reason: String },
    /// No live worker lists `region` or any of `fallback_regions` and can
    /// serve the rest of the request.
    #[error("no worker can place the sandbox in any of the regions: {}", .regions.join(", "))]
    NoWorkerInRegion { regions: Vec<String> },
    /// The operation id was already used for a different request, or another
    /// operation still holds the lease.
    #[error("operation {operation_id} conflicts: {reason}")]
    OperationConflict { operation_id: OperationId, reason: String },
    /// No operation with this id exists for the tenant.
    #[error("operation {operation_id} not found")]
    OperationNotFound { operation_id: OperationId },
    #[error("operation {operation_id} was canceled")]
    OperationCanceled { operation_id: OperationId },
    /// The lease's provider cannot suspend to the requested level. `requested`
    /// is `auto` when the provider offers no suspend at all.
    #[error("lease {lease_id} cannot suspend to level {}: the provider does not offer it", .requested.as_str())]
    SuspendUnsupported {
        lease_id: LeaseId,
        requested: SuspendRequestLevel,
    },
    /// The lease is being woken by another request. The caller retries after
    /// `retry_after_ms`; the wake carries on in the background.
    #[error("lease {lease_id} is waking; retry in {retry_after_ms}ms")]
    LeaseWaking { lease_id: LeaseId, retry_after_ms: u64 },
    /// The artifact a suspended lease needs is gone, so the lease cannot wake.
    #[error("the suspend artifact of lease {lease_id} is lost")]
    SuspendArtifactLost { lease_id: LeaseId },
    /// The persistent sandbox entity store failed. Raised only where the
    /// entity is the source of truth and by the entity verbs.
    #[error("sandbox entity unavailable: {reason}")]
    EntityUnavailable { reason: String },
    /// The broker does not run the entity feature this verb needs.
    #[error("sandbox entities are not enabled on this broker")]
    EntityNotEnabled,
}
