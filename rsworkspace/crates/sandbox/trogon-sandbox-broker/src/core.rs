use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::constants::{DEFAULT_MAX_PAUSED_SECONDS, MAX_LEASE_TTL_SECONDS, MAX_PAUSED_SECONDS_CEILING};
use crate::lease_extension::validate_lease_extension_with_max;
use bytes::Bytes;
use trogon_sandbox_contract::LeaseCounters as LeaseUsageCounters;
use trogon_sandbox_contract::{
    AccountLimits, ArtifactType, CancelIntent, CapabilityProfile, CapabilityRequirements, CheckpointRunningRequest,
    CorrelationId, CreateDriveRequest, CreateDriveResponse, CreateSandboxGroupRequest, CreateSandboxGroupResponse,
    CreateSandboxUserRequest, CreateSandboxUserResponse, DeleteDriveRequest, DeleteDriveResponse, DeleteLeaseRequest,
    DeleteLeaseResponse, DeleteNamedSandboxRequest, DeleteNamedSandboxResponse, DeleteSnapshotTagRequest,
    DeleteSnapshotTagResponse, Digest, DriveError, ExtendLeaseTimeoutRequest, ExtendLeaseTimeoutResponse,
    FreezeInFlight, GetAccountLimitsRequest, GetAccountLimitsResponse, GetLeaseByNameRequest, GetLeaseByNameResponse,
    GetLeaseUsageRequest, GetLeaseUsageResponse, GetSandboxDomainRequest, GetSandboxDomainResponse,
    GetTenantUsageRequest, GetTenantUsageResponse, KillCommandRequest, KillCommandResponse, LeaseError,
    LeaseForkRequest, LeaseGranted, LeaseId, LeaseListRow, LeaseRecord, LeaseReleased, LeaseReleasedReason,
    LeaseRequest, LeaseResponse, LeaseSnapshotReceipt, LeaseStatus, LeaseSummary, ListCapabilitiesRequest,
    ListCapabilitiesResponse, ListDrivesRequest, ListDrivesResponse, ListLeasesRequest, ListLeasesResponse,
    ListProcessesRequest, ListProcessesResponse, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse,
    NativeHandleRequest, NativeHandleResponse, NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse, OciArtifactRef,
    OperationId, PauseLeaseRequest, PauseLeaseResponse, PollCommandRequest, PollCommandResponse, ProviderKind,
    PurgeNamedRequest, PurgeNamedResponse, ReadFileRequest, ReadFileResponse, ReadLogsRequest, ReadLogsResponse,
    ReapFailure, ReapLeasesRequest, ReapLeasesResponse, ResourceGauges, ResourceUsageRequest, ResourceUsageResponse,
    RestartCommandRequest, RestartCommandResponse, ResumeLeaseRequest, ResumeLeaseResponse, RunCommandRequest,
    RunCommandResponse, SandboxFreezeRequest, SandboxFreezeResponse, SandboxImage, SandboxTags, ServiceAccountId,
    SetSnapshotTagRequest, SetSnapshotTagResponse, SnapshotDeleteRequest, SnapshotDeleteResponse, SnapshotDeleted,
    SnapshotGetRequest, SnapshotGetResponse, SnapshotKvRecord, SnapshotLeaseRequest, SnapshotLeaseResponse,
    SnapshotListRow, SnapshotPushTarget, SnapshotTag, SnapshotTagAuditEntry, SnapshotTagAuditEvent,
    SnapshotTagHistoryRequest, SnapshotTagHistoryResponse, SnapshotTagRecord, SnapshotTreeNode, SnapshotTreeRequest,
    SnapshotTreeResponse, SshAccessRequest, SshAccessResponse, StdinMode, StopCommandRequest, StopCommandResponse,
    StopLeaseRequest, StopLeaseResponse, TagLimits, TenantId, TenantUsage, UpdateLeaseTagsRequest,
    UpdateLeaseTagsResponse, WaitReadyRequest, WaitReadyResponse, WaitSandboxReadyRequest, WaitSandboxReadyResponse,
    WriteFilesRequest, WriteFilesResponse, WriteStdinRequest, WriteStdinResponse,
};
use trogon_sandbox_contract::{FileOpRequest, FileOpResponse};
use trogon_sandbox_contract::{
    GetOperationRequest, GetOperationResponse, OperationRecord, OperationRequest, OperationResult, OperationState,
};
use trogon_sandbox_contract::{
    ProviderDispatchReply, ProviderLease as CoreProviderLease, SnapshotExportOutcome, SnapshotReceipt,
};
use trogon_sandbox_contract::{SnapshotCaptureAccess, SnapshotTagName};
use trogon_sandbox_contract::{TerminalOp, TerminalRequest, TerminalResponse};
use trogon_sandbox_protocol::snapshot_repository_for_tenant;
use trogon_sandbox_protocol::subjects;

use self::operation_driver::unexpected_result;

use tracing::{debug, error, info, warn};

use crate::active_lease::ActiveLeaseStore;
use crate::bus::BrokerBus;
use crate::command_launch::{Claim, CommandLaunch, CommandLaunchStore, InMemoryCommandLaunchStore, LaunchState};
use crate::digest_pinning::SnapshotPinStore;
use crate::drive_purge::{DriveDataPurger, NoopDriveDataPurger};
use crate::drive_store::{ClaimPage, DriveClaim, DriveStore, DriveStoreError};
use crate::host_import_policy::HostImportPolicy;
use crate::host_mount_policy::{HostMountAllowList, path_prefix_matches};
use crate::idempotency::IdempotencyStore;
use crate::image_policy::ImageAllowList;
use crate::image_readiness::{ImageReadinessChecker, NoopImageReadinessChecker};
use crate::limits::TenantLimits;
use crate::metrics::BrokerMetrics;
use crate::named_lease::{NameClaim, NamedLeaseStore};
use crate::native_policy::{DenyNativePassthrough, NativePassthroughPolicy};
use crate::network_policy_validation::validate_network_policy;
use crate::pending_purge::{InMemoryPendingPurgeStore, PendingPurge, PendingPurgeStore, PendingRefund};
use crate::pending_release::{InMemoryPendingReleaseStore, PendingRelease, PendingReleaseStore};
use crate::quota::QuotaStore;
use crate::resolved_digest::ResolvedDigestStore;
use crate::snapshot_last_used::{NoopSnapshotLastUsedStore, SnapshotLastUsedStore};
use crate::snapshot_location::{SnapshotManifestProbe, locate_snapshot_repository};
use crate::snapshot_tag::{SnapshotTagStore, SnapshotTagStoreError};
use crate::usage_store::{InMemoryLeaseUsageStore, LeaseUsageStore, UsageEntry};
use crate::worker_registry::{
    ProviderSelection, ProviderSelectionError, WorkerRegistry, WorkerSelectionConstraints, select_provider_in_regions,
};
use trogon_sandbox_contract::WorkspaceSource;
use trogon_sandbox_oci::ImageReadiness;

use crate::image_labels::SandboxImageLabels;
use crate::placement_gate::{derive_env_digest, egress_requirements_only};
use crate::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_contract::{
    CapabilityNeed, FreezeAction, LeaseCreation, LeasePersistence, LeasePrewarm, LeaseRequestKey, SandboxId,
};

mod broker_config;
mod constants;
mod dispatch_payload_error;
mod helpers;
mod in_memory_drive_store;
mod in_memory_snapshot_kv_reader;
mod in_memory_snapshot_tag_history_reader;
mod jetstream_snapshot_tag_history_reader;
mod lifetime_policy;
mod lock;
mod nats_kv_snapshot_kv_reader;
mod noop_snapshot_kv_reader;
mod noop_snapshot_tag_history_reader;
mod snapshot_kv_error;
mod snapshot_kv_reader;
mod snapshot_tag_history_error;
mod snapshot_tag_history_reader;
mod spec_validation_error;

pub use self::broker_config::BrokerConfig;
pub use self::constants::{
    DEFAULT_ORPHAN_CLAIM_GRACE, FREEZE_IN_FLIGHT_STALE_AFTER, FREEZE_SETTLE_MAX_ATTEMPTS, PENDING_PURGE_RECONCILE_AFTER,
};
pub use self::dispatch_payload_error::DispatchPayloadError;
pub use self::dispatch_signing_error::DispatchSigningError;
pub use self::dispatch_transaction_error::DispatchTransactionError;
pub use self::helpers::lease_released_msg_id;
pub use self::in_memory_drive_store::InMemoryDriveStore;
pub use self::in_memory_snapshot_kv_reader::InMemorySnapshotKvReader;
pub use self::in_memory_snapshot_tag_history_reader::InMemorySnapshotTagHistoryReader;
pub use self::jetstream_snapshot_tag_history_reader::JetStreamSnapshotTagHistoryReader;
pub use self::lease_release_error::LeaseReleaseError;
pub use self::lifetime_policy::LifetimePolicy;
pub use self::nats_kv_snapshot_kv_reader::NatsKvSnapshotKvReader;
pub use self::noop_snapshot_kv_reader::NoopSnapshotKvReader;
pub use self::noop_snapshot_tag_history_reader::NoopSnapshotTagHistoryReader;
pub use self::snapshot_kv_error::SnapshotKvError;
pub use self::snapshot_kv_reader::SnapshotKvReader;
pub use self::snapshot_tag_history_error::SnapshotTagHistoryError;
pub use self::snapshot_tag_history_reader::SnapshotTagHistoryReader;

use self::constants::{
    LAUNCH_JOURNAL_MAX_REPLY_BYTES, LAUNCH_KEY_LOST_GRACE, LAUNCH_KEY_POLL_INTERVAL, LAUNCH_KEY_REPLAY_WINDOW,
    MAX_SNAPSHOT_TREE_DEPTH, ORPHAN_CLAIM_RELEASES_PER_SWEEP, ORPHAN_CLAIM_SCAN_LIMIT, PORT_PROBE_MAX_OUTPUT_BYTES,
    PORT_PROBE_NO_TOOL_EXIT, PORT_PROBE_SCRIPT, PORT_PROBE_TIMEOUT_SECONDS, PROVIDER_DETACHED_START_TIMEOUT,
    PROVIDER_RUN_COMMAND_GRACE, PROVIDER_TERMINAL_TIMEOUT, PROVIDER_WRITE_STDIN_TIMEOUT, PURGE_NAMED_TIMEOUT,
    PURGE_TAG_DELETE_ATTEMPTS, PURGE_TAG_DELETE_BACKOFF, SANDBOX_READY_POLL_INTERVAL,
    SNAPSHOT_TAG_MUTATION_MAX_RETRIES,
};
use self::helpers::{
    announcement, backend, build_dispatch_payload, decode_dispatch_reply, detached_start_timeout, elapsed_at_least,
    foreground_run_command_timeout, freeze_settle_backoff, is_no_responders, is_request_timeout, lease_summary,
    lost_launch_reply, native_for_provider, region_order, retired_launch_reply, substrate_unsupported_spec_reason,
    unkept_launch_reply, validate_regions, validate_spec,
};

/// Claim identity to its first sighting and the scan cycle that last saw it.
type OrphanClaimsSeen = std::collections::HashMap<(trogon_sandbox_contract::DriveId, LeaseId, u64), (SystemTime, u64)>;

#[derive(Default)]
struct OrphanClaimScan {
    cursor: Option<String>,
    cycle: u64,
}

mod activity;
mod command_readiness;
mod commands;
mod create_operation;
mod create_operation_error;
mod credential_policy;
mod dispatch_reply_error;
mod dispatch_signing;
mod dispatch_signing_error;
mod dispatch_transaction;
mod dispatch_transaction_error;
mod dormancy;
mod drives;
mod entity_api;
mod entity_hooks;
mod entity_intent;
mod entity_reconciler;
mod files;

mod assignment_epoch;
mod capability_inference;
use self::capability_inference::infer_capability_requirements;
mod generated_ids;
mod spec_edit;
use self::assignment_epoch::epoch_for_assignment;
use self::spec_edit::{with_bucket_backed_drives, with_image, with_network_policy, with_remote_drive_mounts};
mod persistent_snapshot;
use self::persistent_snapshot::{PersistentSnapshotOptions, with_persistence};
mod lease_delete;
mod lease_listing;
mod lease_mutations;
mod lease_release_error;
mod lease_request;
mod operation_cleanup;
mod operation_cleanup_error;
mod operation_driver;
mod pause_resume;
mod region_error;
mod release;
mod rollback;
mod snapshots;
mod terminal_access;
mod usage;
mod users_groups;
use self::create_operation::CreateCtx;
use self::generated_ids::{generated_attempt_id, generated_correlation_id, generated_lease_id, generated_operation_id};

/// The lease id a named create provisions under. Deriving it from the
/// idempotency key makes a retried create find its own name claim (and any
/// lease it already provisioned) instead of conflicting with itself.
pub(crate) fn derived_named_lease_id(tenant: &TenantId, name: &str, idempotency_key: &str) -> LeaseId {
    generated_ids::lease_id_from_parts([tenant.as_str(), name, idempotency_key])
}

/// What a worker kept when it checkpointed a running lease.
enum CheckpointedSource {
    Oci(SnapshotReceipt),
    Native(trogon_sandbox_contract::NativeSnapshotReceipt),
}

enum NameClaimOutcome {
    Claimed,
    Existing(LeaseGranted),
}

/// Outcome of one port probe.
enum PortProbe {
    Open,
    Closed(Vec<u16>),
    Unsupported,
    Denied(LeaseError),
    /// The probe itself failed in a way that may pass on the next attempt.
    Retry,
}

pub struct BrokerCore {
    dispatch_signer: Option<Arc<trogon_sandbox_protocol::DispatchSigner>>,
    bus: Arc<dyn BrokerBus>,
    quota: Arc<dyn QuotaStore>,
    idempotency: Arc<dyn IdempotencyStore>,
    active_leases: Arc<dyn ActiveLeaseStore>,
    image_allowlist: Arc<dyn ImageAllowList>,
    host_mount_allowlist: Arc<dyn HostMountAllowList>,
    host_import_policy: Arc<dyn HostImportPolicy>,
    /// Which tenants may set `platform.container`; denies all until configured.
    hardening_override_policy: crate::hardening_override_policy::HardeningOverridePolicy,
    tenant_limits: Arc<dyn TenantLimits>,
    worker_registry: Arc<dyn WorkerRegistry>,
    snapshot_pins: Arc<dyn SnapshotPinStore>,
    tag_store: Arc<dyn SnapshotTagStore>,
    resolved_digests: Arc<dyn ResolvedDigestStore>,
    image_readiness: Arc<dyn ImageReadinessChecker>,
    snapshot_last_used: Arc<dyn SnapshotLastUsedStore>,
    /// Which tenant took each provider-native snapshot; a restore is limited
    /// to the tenant's own.
    native_snapshots: Arc<dyn crate::native_snapshot::NativeSnapshotStore>,
    usage: Arc<dyn LeaseUsageStore>,
    drive_store: Arc<dyn DriveStore>,
    drive_data_purger: Arc<dyn DriveDataPurger>,
    named_leases: Arc<dyn NamedLeaseStore>,
    snapshot_kv: Arc<dyn SnapshotKvReader>,
    tag_history: Arc<dyn SnapshotTagHistoryReader>,
    /// Registry deletion behind `snapshot_delete`; `None` replies Unsupported.
    snapshot_remover: Option<Arc<dyn crate::snapshot_tracker::SnapshotRemover>>,
    /// Registry probe that finds snapshots still in a tenant's legacy
    /// repository; `None` restores from the current repository only.
    snapshot_manifest_probe: Option<Arc<dyn SnapshotManifestProbe>>,
    /// Retained command output behind `read_logs`; `None` replies Unsupported.
    command_logs: Option<Arc<dyn crate::command_logs::CommandLogStore>>,
    /// Launch keys already dispatched per lease, so a retried run_command
    /// replays instead of running twice. In memory unless a shared store is set.
    command_launches: Arc<dyn CommandLaunchStore>,
    pending_purges: Arc<dyn PendingPurgeStore>,
    pending_releases: Arc<dyn PendingReleaseStore>,
    /// Where each lease's credential policy is committed for the credential
    /// broker's fence; `None` commits nothing.
    committed_policies: Option<Arc<dyn trogon_sandbox_credential_broker::CommittedPolicyWriter>>,
    /// Which tenants may use vendor passthrough; denies everyone by default.
    native_policy: Arc<dyn NativePassthroughPolicy>,
    /// Shadow mirror into the persistent entity store; `None` when the mode
    /// is `off`, which keeps every lease path exactly as it was.
    entity: Option<Arc<crate::entity_shadow::EntityShadow>>,
    config: BrokerConfig,
    /// Ceiling for a command's per-stream output retention; requests asking
    /// for more are clamped to it before reaching a worker.
    max_command_output_bytes: usize,
    lifetime: LifetimePolicy,
    metrics: BrokerMetrics,
    attempt_locks: std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// Dispatches whose outcome stayed unknown (reply lost or undecodable),
    /// keyed by lease id, so the reconciler can re-send the identical attempt.
    pending_redrives: std::sync::Mutex<std::collections::HashMap<String, PendingRedrive>>,
    /// Records pause, resume and delete so a restart or a lost reply keeps
    /// their outcome. `None` runs them inline with no record.
    operations: Option<Arc<crate::operations::Operations>>,
    /// Operations a task in this process is driving right now, by record key.
    driving: std::sync::Mutex<std::collections::HashSet<String>>,
    orphan_claim_grace: Duration,
    /// First sighting of each drive claim the orphan sweep is waiting on, for
    /// stores that do not report when a claim was written. Entries not
    /// seen for two scan cycles are dropped.
    orphan_claims_seen: std::sync::Mutex<OrphanClaimsSeen>,
    /// Where the next orphan sweep resumes. Each sweep examines at most
    /// [`ORPHAN_CLAIM_SCAN_LIMIT`] drives, so the cursor walks the whole
    /// store over successive sweeps instead of starving what lies past the
    /// first page.
    orphan_claim_scan: std::sync::Mutex<OrphanClaimScan>,
    /// Handle to this core, bound once it is shared, so a verb that meets a
    /// suspended lease can start its wake in the background.
    self_handle: std::sync::OnceLock<std::sync::Weak<BrokerCore>>,
}

/// An unresolved targeted dispatch kept for re-sending to its worker.
struct PendingRedrive {
    payload: Bytes,
    last_sent: SystemTime,
}

impl BrokerCore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bus: Arc<dyn BrokerBus>,
        quota: Arc<dyn QuotaStore>,
        idempotency: Arc<dyn IdempotencyStore>,
        active_leases: Arc<dyn ActiveLeaseStore>,
        image_allowlist: Arc<dyn ImageAllowList>,
        host_mount_allowlist: Arc<dyn HostMountAllowList>,
        host_import_policy: Arc<dyn HostImportPolicy>,
        tenant_limits: Arc<dyn TenantLimits>,
        worker_registry: Arc<dyn WorkerRegistry>,
        snapshot_pins: Arc<dyn SnapshotPinStore>,
        tag_store: Arc<dyn SnapshotTagStore>,
        resolved_digests: Arc<dyn ResolvedDigestStore>,
        drive_store: Arc<dyn DriveStore>,
        named_leases: Arc<dyn NamedLeaseStore>,
        config: BrokerConfig,
        metrics: BrokerMetrics,
    ) -> Self {
        Self {
            bus,
            quota,
            idempotency,
            active_leases,
            image_allowlist,
            host_mount_allowlist,
            host_import_policy,
            tenant_limits,
            worker_registry,
            snapshot_pins,
            tag_store,
            resolved_digests,
            image_readiness: Arc::new(NoopImageReadinessChecker),
            snapshot_last_used: Arc::new(NoopSnapshotLastUsedStore),
            native_snapshots: Arc::new(crate::native_snapshot::InMemoryNativeSnapshotStore::new()),
            usage: Arc::new(InMemoryLeaseUsageStore::new()),
            drive_store,
            drive_data_purger: Arc::new(NoopDriveDataPurger),
            named_leases,
            snapshot_kv: Arc::new(InMemorySnapshotKvReader::new()),
            tag_history: Arc::new(NoopSnapshotTagHistoryReader),
            snapshot_remover: None,
            snapshot_manifest_probe: None,
            command_logs: None,
            command_launches: Arc::new(InMemoryCommandLaunchStore::new()),
            pending_purges: Arc::new(InMemoryPendingPurgeStore::new()),
            pending_releases: Arc::new(InMemoryPendingReleaseStore::new()),
            committed_policies: None,
            orphan_claim_grace: DEFAULT_ORPHAN_CLAIM_GRACE,
            orphan_claims_seen: Default::default(),
            orphan_claim_scan: Default::default(),
            self_handle: Default::default(),
            hardening_override_policy: Default::default(),
            native_policy: Arc::new(DenyNativePassthrough),
            entity: None,
            config,
            max_command_output_bytes: Self::DEFAULT_MAX_COMMAND_OUTPUT_BYTES,
            lifetime: LifetimePolicy::default(),
            metrics,
            attempt_locks: std::sync::Mutex::new(std::collections::HashMap::new()),
            pending_redrives: std::sync::Mutex::new(std::collections::HashMap::new()),
            operations: None,
            dispatch_signer: None,
            driving: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub const DEFAULT_MAX_COMMAND_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

    pub fn with_max_command_output_bytes(mut self, ceiling: usize) -> Self {
        self.max_command_output_bytes = ceiling;
        self
    }

    pub fn with_lifetime_policy(mut self, policy: LifetimePolicy) -> Self {
        self.lifetime = policy;
        self
    }

    pub fn with_image_readiness(mut self, checker: Arc<dyn ImageReadinessChecker>) -> Self {
        self.image_readiness = checker;
        self
    }

    /// Shares per-lease usage counters across broker replicas.
    pub fn with_usage_store(mut self, store: Arc<dyn LeaseUsageStore>) -> Self {
        self.usage = store;
        self
    }

    pub fn with_native_snapshot_store(mut self, store: Arc<dyn crate::native_snapshot::NativeSnapshotStore>) -> Self {
        self.native_snapshots = store;
        self
    }

    pub fn with_snapshot_last_used(mut self, store: Arc<dyn SnapshotLastUsedStore>) -> Self {
        self.snapshot_last_used = store;
        self
    }

    /// Keeps unfinished delete-by-id purges durable across broker restarts.
    pub fn with_pending_purge_store(mut self, store: Arc<dyn PendingPurgeStore>) -> Self {
        self.pending_purges = store;
        self
    }

    /// How old a drive claim must be before the sweep may release it when no
    /// lease backs it. Raised to twice the provider dispatch timeout when
    /// shorter, so the sweep never outpaces a slow grant.
    pub fn with_orphan_drive_claim_grace(mut self, grace: Duration) -> Self {
        self.orphan_claim_grace = grace;
        self
    }

    /// Keeps the follow-up of a release that is not a purge durable across
    /// broker restarts.
    pub fn with_pending_release_store(mut self, store: Arc<dyn PendingReleaseStore>) -> Self {
        self.pending_releases = store;
        self
    }

    pub fn with_drive_data_purger(mut self, purger: Arc<dyn DriveDataPurger>) -> Self {
        self.drive_data_purger = purger;
        self
    }

    pub fn with_snapshot_kv_reader(mut self, reader: Arc<dyn SnapshotKvReader>) -> Self {
        self.snapshot_kv = reader;
        self
    }

    pub fn with_snapshot_manifest_probe(mut self, probe: Arc<dyn SnapshotManifestProbe>) -> Self {
        self.snapshot_manifest_probe = Some(probe);
        self
    }

    /// Repository to restore `digest` of `tenant` from, which is the legacy
    /// repository only for snapshots pushed there before the current layout.
    pub async fn snapshot_repository_for_restore(
        &self,
        tenant: &TenantId,
        digest: &Digest,
        artifact_type: ArtifactType,
    ) -> String {
        locate_snapshot_repository(
            self.snapshot_manifest_probe.as_deref(),
            &self.config.snapshot_registry,
            tenant,
            digest,
            artifact_type,
        )
        .await
    }

    pub fn with_snapshot_remover(mut self, remover: Arc<dyn crate::snapshot_tracker::SnapshotRemover>) -> Self {
        self.snapshot_remover = Some(remover);
        self
    }

    pub fn with_snapshot_tag_history_reader(mut self, reader: Arc<dyn SnapshotTagHistoryReader>) -> Self {
        self.tag_history = reader;
        self
    }

    pub fn with_hardening_override_policy(
        mut self,
        policy: crate::hardening_override_policy::HardeningOverridePolicy,
    ) -> Self {
        self.hardening_override_policy = policy;
        self
    }

    pub fn with_command_logs(mut self, store: Arc<dyn crate::command_logs::CommandLogStore>) -> Self {
        self.command_logs = Some(store);
        self
    }

    /// Shares the launch-key journal across broker replicas, so a retry that
    /// lands on another replica still replays the first launch.
    pub fn with_command_launch_store(mut self, store: Arc<dyn CommandLaunchStore>) -> Self {
        self.command_launches = store;
        self
    }

    /// Sets which tenants may send `LeaseRequest::native` and read native
    /// handles. Without it every tenant is denied.
    pub fn with_native_policy(mut self, policy: Arc<dyn NativePassthroughPolicy>) -> Self {
        self.native_policy = policy;
        self
    }

    /// Makes pause, resume and delete durable operations kept in `operations`.
    pub fn with_operations(mut self, operations: Arc<crate::operations::Operations>) -> Self {
        self.operations = Some(operations);
        self
    }

    pub fn metrics(&self) -> &BrokerMetrics {
        &self.metrics
    }

    pub fn quota(&self) -> Arc<dyn QuotaStore> {
        Arc::clone(&self.quota)
    }

    /// Tenant owning an active lease, so the transport layer can authenticate
    /// a lease-scoped request against it.
    pub async fn lease_tenant(&self, lease_id: &LeaseId) -> Option<TenantId> {
        match self.active_leases.get(lease_id).await {
            Ok(record) => record.map(|r| r.tenant),
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "lease tenant lookup failed");
                None
            }
        }
    }

    /// Lists provider capability profiles known to the worker registry,
    /// optionally filtered to a single provider. Only providers with a
    /// currently live (non-stale) heartbeat are represented, since a stale
    /// or absent provider's profile is not something a tenant could actually
    /// lease against right now.
    pub async fn handle_list_capabilities(&self, req: ListCapabilitiesRequest) -> ListCapabilitiesResponse {
        let profiles = self
            .worker_registry
            .list_capability_profiles()
            .await
            .iter()
            .filter(|p| req.provider.is_none_or(|want| p.provider == want))
            .map(CapabilityProfile::from)
            .collect();
        ListCapabilitiesResponse { profiles }
    }

    /// Fetches a lease the tenant owns. A lease owned by another tenant reads
    /// as not found.
    async fn owned_lease(&self, tenant: &TenantId, lease_id: &LeaseId) -> Result<LeaseRecord, LeaseError> {
        match self.active_leases.get(lease_id).await {
            Ok(Some(r)) if r.tenant == *tenant => Ok(r),
            Ok(_) => Err(LeaseError::LeaseNotFound {
                lease_id: lease_id.clone(),
            }),
            Err(e) => Err(LeaseError::Backend { message: e.to_string() }),
        }
    }

    async fn store_lease_update(&self, before: &LeaseRecord, after: &LeaseRecord) -> Result<(), LeaseError> {
        self.update_record(before, after).await.map_err(backend)
    }
}

/// What came back from dispatching a run_command to its worker.
enum Dispatched {
    /// The worker answered, so whether the command ran is known.
    Reply(RunCommandResponse),
    /// No usable answer: the command may or may not have run.
    Unknown(RunCommandResponse),
    /// No worker received the request, so the command did not run.
    NotDelivered(RunCommandResponse),
}

impl Dispatched {
    fn response(self) -> RunCommandResponse {
        match self {
            Self::Reply(response) | Self::Unknown(response) | Self::NotDelivered(response) => response,
        }
    }
}

#[cfg(test)]
mod tests;
