use clap::Parser;
use trogon_sandbox_contract::TenantId;
use trogon_sandbox_observability::ObservabilityArgs;

use super::{IdempotencyBackend, QuotaBackend};
use crate::command_logs::CommandLogRetention;
use crate::committed_policies;
use crate::core::BrokerCore;
use crate::entity_store::SandboxEntityMode;

#[derive(Parser, Debug)]
#[command(name = "trogon-sandbox-broker")]
pub struct Cli {
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// Tenants allowed to loosen container hardening per lease through
    /// `platform.container`. Empty denies every tenant.
    #[arg(long, env = "SANDBOX_HARDENING_OVERRIDE_TENANTS", value_delimiter = ',')]
    pub hardening_override_tenants: Vec<TenantId>,

    #[arg(long, env = "SANDBOX_DEFAULT_CPU_CAP", default_value_t = 3600)]
    pub default_cpu_cap: u32,

    #[arg(long, env = "SANDBOX_DEFAULT_WALL_CAP", default_value_t = 7200)]
    pub default_wall_cap: u32,

    #[arg(long, env = "SANDBOX_QUOTA_BACKEND", default_value = "nats-kv", value_enum)]
    pub quota_backend: QuotaBackend,

    #[arg(long, env = "SANDBOX_QUOTA_BUCKET", default_value = "sandbox-quota")]
    pub quota_bucket: String,

    #[arg(long, env = "SANDBOX_IDEMPOTENCY_BACKEND", default_value = "nats-kv", value_enum)]
    pub idempotency_backend: IdempotencyBackend,

    #[arg(long, env = "SANDBOX_IDEMPOTENCY_BUCKET", default_value = "sandbox-idempotency")]
    pub idempotency_bucket: String,

    #[arg(long, env = "SANDBOX_ACTIVE_LEASE_BUCKET", default_value = "sandbox-active-leases")]
    pub active_lease_bucket: String,

    /// Milliseconds between orphan-lease reclamation sweeps.
    ///
    /// # Known limitation (multi-broker HA)
    ///
    /// Legacy-provider sweeps can double-refund when multiple brokers process
    /// the same expiration. Targeted microVM attempts use durable accounting
    /// and cleanup receipts.
    #[arg(long, env = "SANDBOX_RECLAMATION_TICK_MS", default_value_t = 5000)]
    pub reclamation_tick_ms: u64,

    /// Milliseconds between snapshot retention/eviction sweeps.
    ///
    /// This is low-urgency background cleanup (unlike the ~5s reclamation sweeper or the
    /// 10s prewarm tick), so it defaults to a much longer interval.
    #[arg(long, env = "SANDBOX_RETENTION_TICK_MS", default_value_t = 300_000)]
    pub retention_tick_ms: u64,

    /// Comma-separated tenant IDs the retention sweeper considers each tick, in
    /// addition to any found by `--retention-sweep-discover-tenants`. With
    /// neither set the tag sweep has nothing to do.
    #[arg(long, env = "SANDBOX_RETENTION_SWEEP_TENANTS", value_delimiter = ',')]
    pub retention_sweep_tenants: Vec<TenantId>,

    /// Also sweep every tenant that owns a snapshot tag, found from the
    /// tenant-scoped keys of the tag store, so no fixed list is needed. Off by
    /// default: turning it on starts enforcing tag retention policies for all
    /// tenants, not just the listed ones.
    #[arg(long, env = "SANDBOX_RETENTION_SWEEP_DISCOVER_TENANTS", default_value_t = false)]
    pub retention_sweep_discover_tenants: bool,

    /// Snapshots kept per tenant and source lease by the catalog sweep. The
    /// rest are deleted from the registry and the catalog, except restore
    /// sources of live or named leases and digests a tag points at. Zero (the
    /// default) turns the keep-N pass off.
    #[arg(long, env = "SANDBOX_SNAPSHOT_KEEP_PER_LEASE", default_value_t = 0)]
    pub snapshot_keep_per_lease: usize,

    /// Delete registry snapshot artifacts that have no catalog record, found
    /// through the registry tag list. Off by default; see the orphan grace.
    #[arg(long, env = "SANDBOX_SNAPSHOT_ORPHAN_SWEEP", default_value_t = false)]
    pub snapshot_orphan_sweep: bool,

    /// Seconds an artifact must go without a catalog record before the orphan
    /// pass may delete it. Only effective with `--snapshot-orphan-sweep`.
    #[arg(long, env = "SANDBOX_SNAPSHOT_ORPHAN_GRACE_SECS", default_value_t = 86_400)]
    pub snapshot_orphan_grace_secs: u64,

    /// Development-only switch: turns off bearer-key authentication, the image allow-list
    /// and the worker registry in one go, and logs a warning at startup. Without it all
    /// three are enforced, which needs `SPICEDB_ENDPOINT` and `SPICEDB_TOKEN`.
    #[arg(long, env = "SANDBOX_INSECURE_DEV", default_value_t = false)]
    pub insecure_dev: bool,

    #[arg(long, env = "SANDBOX_AUTHZ_ENFORCE", default_value_t = true)]
    pub authz_enforce: bool,

    #[command(flatten)]
    pub committed_policies: committed_policies::Args,

    #[arg(
        long,
        env = "SANDBOX_COSIGN_PUBLIC_KEY",
        help = "PEM path for the Cosign verification key (derived from the KMS signing key; workers use the same path)"
    )]
    pub cosign_public_key: Option<std::path::PathBuf>,

    #[arg(long, env = "SPICEDB_ENDPOINT")]
    pub spicedb_endpoint: Option<String>,

    #[arg(long, env = "SPICEDB_TOKEN")]
    pub spicedb_token: Option<String>,

    #[arg(long, env = "SANDBOX_SPICEDB_TENANT_RESOURCE_TYPE", default_value = "sandbox_tenant")]
    pub spicedb_tenant_resource_type: trogon_sandbox_auth::TenantResourceType,

    #[arg(
        long,
        env = "SANDBOX_API_KEY_BUCKET",
        default_value = trogon_sandbox_auth::DEFAULT_BUCKET
    )]
    pub api_key_bucket: trogon_sandbox_auth::ApiKeyBucket,

    #[arg(long, env = "SANDBOX_ENFORCE_IMAGE_ALLOWLIST", default_value_t = true)]
    pub enforce_image_allowlist: bool,

    #[arg(
        long,
        env = "SANDBOX_IMAGE_ALLOWLIST_BUCKET",
        default_value = "sandbox-image-allowlist"
    )]
    pub image_allowlist_bucket: String,

    #[arg(
        long,
        env = "SANDBOX_HOST_MOUNT_ALLOWLIST_BUCKET",
        default_value = "sandbox-host-mount-allowlist"
    )]
    pub host_mount_allowlist_bucket: String,

    /// KV bucket of per-tenant native passthrough records, written by
    /// the admin tool's native-passthrough command.
    #[arg(
        long,
        env = "SANDBOX_NATIVE_PASSTHROUGH_BUCKET",
        default_value = "sandbox-native-passthrough"
    )]
    pub native_passthrough_bucket: String,

    #[arg(long, env = "SANDBOX_TENANT_LIMITS_BUCKET", default_value = "sandbox-tenant-limits")]
    pub tenant_limits_bucket: String,

    #[arg(long, env = "SANDBOX_DEFAULT_MAX_CPU_PER_REQUEST", default_value_t = 3600)]
    pub default_max_cpu_seconds_per_request: u32,

    #[arg(long, env = "SANDBOX_DEFAULT_MAX_MEMORY_MIB_PER_REQUEST", default_value_t = 8192)]
    pub default_max_memory_mib_per_request: u32,

    #[arg(long, env = "SANDBOX_ENFORCE_WORKER_REGISTRY", default_value_t = true)]
    pub enforce_worker_registry: bool,

    #[arg(long, env = "SANDBOX_WORKER_HEARTBEAT_TTL_SECS", default_value_t = 30)]
    pub worker_heartbeat_ttl_secs: u64,

    #[arg(long, env = "SANDBOX_DEFAULT_ACTIVE_LEASE_CAP", default_value_t = 50)]
    pub default_active_lease_cap: u32,

    #[arg(long, env = "SANDBOX_DEFAULT_SNAPSHOT_COUNT_CAP", default_value_t = 100)]
    pub default_snapshot_count_cap: u64,

    #[arg(
        long,
        env = "SANDBOX_DEFAULT_SNAPSHOT_BYTES_CAP",
        default_value_t = 100 * 1024 * 1024 * 1024
    )]
    pub default_snapshot_bytes_cap: u64,

    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = "snapshots.trogon.internal")]
    pub snapshot_registry: String,

    /// Deadline for the broker-to-provider dispatch round trip. A cold golden-snapshot
    /// build on the substrate provider can take far longer than a typical request-reply
    /// exchange, so this defaults far above the transport's own default request timeout.
    #[arg(long, env = "SANDBOX_PROVIDER_DISPATCH_TIMEOUT_SECS", default_value_t = 600)]
    pub provider_dispatch_timeout_secs: u64,

    /// Age past which a drive claim no lease backs is released by the sweeper.
    /// Raised to twice the dispatch timeout when shorter, since a grant claims
    /// its drives before it writes the lease record.
    #[arg(long, env = "SANDBOX_ORPHAN_DRIVE_CLAIM_GRACE_SECS", default_value_t = 1800)]
    pub orphan_drive_claim_grace_secs: u64,

    /// Ceiling on how many bytes of each of a command's stdout and stderr a
    /// worker may retain; a request's `max_output_bytes` is clamped to it.
    #[arg(
        long,
        env = "SANDBOX_MAX_COMMAND_OUTPUT_BYTES",
        default_value_t = BrokerCore::DEFAULT_MAX_COMMAND_OUTPUT_BYTES
    )]
    pub max_command_output_bytes: usize,

    /// Longest a lease may live, initial TTL plus every extension. Raise it
    /// above a day only when every provider behind the broker can hold a
    /// sandbox that long or advertises its own cap.
    #[arg(
        long,
        env = "SANDBOX_MAX_LEASE_LIFETIME_SECONDS",
        default_value_t = crate::constants::MAX_LEASE_TTL_SECONDS
    )]
    pub max_lease_lifetime_seconds: u32,

    /// Lets a lease ask for `no_ttl`, ending only on release or its idle timeout.
    #[arg(long, env = "SANDBOX_ALLOW_NO_TTL_LEASES", default_value_t = false)]
    pub allow_no_ttl_leases: bool,

    #[arg(long, env = "SANDBOX_JETSTREAM_REPLICAS", default_value_t = 3)]
    pub jetstream_replicas: usize,

    /// How the persistent sandbox entity store is used: `shadow` (default)
    /// mirrors named persistent leases into it without affecting any response,
    /// so the store fills and repairs while the legacy paths stay
    /// authoritative; `off` never creates its buckets or stream and leaves
    /// every lease path untouched; `on` makes the entity the source of truth,
    /// fails a request with a typed error when the entity store fails, and
    /// enables entity delete. `on` stays opt-in until the soak passes.
    #[arg(long, env = "SANDBOX_ENTITY_MODE", default_value = "shadow")]
    pub entity_mode: SandboxEntityMode,

    /// Milliseconds between entity repair sweeps. Ignored when the entity
    /// mode is `off`.
    #[arg(long, env = "SANDBOX_ENTITY_SWEEP_TICK_MS", default_value_t = 30_000)]
    pub entity_sweep_tick_ms: u64,

    /// Run the entity reconciler on each sweep tick. Only acts in mode `on`.
    #[arg(long, env = "SANDBOX_ENTITY_RECONCILE", default_value_t = true, action = clap::ArgAction::Set)]
    pub entity_reconcile: bool,

    /// Total bytes of streamed command output kept for `read_logs`, across all
    /// executions. The oldest output is dropped first.
    #[arg(
        long,
        env = "SANDBOX_COMMAND_LOG_MAX_BYTES",
        default_value_t = CommandLogRetention::default().max_bytes
    )]
    pub command_log_max_bytes: i64,

    /// Longest streamed command output is kept for `read_logs`, in seconds.
    #[arg(long, env = "SANDBOX_COMMAND_LOG_MAX_AGE_SECS", default_value_t = 86_400)]
    pub command_log_max_age_secs: u64,

    /// Longest a command's exit record is kept, in seconds, so a late reader
    /// still learns the exit after the output aged out. Raised to the output
    /// age when lower.
    #[arg(long, env = "SANDBOX_COMMAND_EXIT_MAX_AGE_SECS", default_value_t = 604_800)]
    pub command_exit_max_age_secs: u64,

    /// Most output chunks kept per execution for `read_logs`.
    #[arg(
        long,
        env = "SANDBOX_COMMAND_LOG_MAX_CHUNKS_PER_EXECUTION",
        default_value_t = CommandLogRetention::default().max_chunks_per_execution
    )]
    pub command_log_max_chunks_per_execution: i64,

    /// Tenants always allowed to send `native` vendor options and read native
    /// handles, whatever their stored record says. Other tenants need an
    /// allowing record in the native passthrough bucket.
    #[arg(long, env = "SANDBOX_NATIVE_PASSTHROUGH_TENANTS", value_delimiter = ',')]
    pub native_passthrough_tenants: Vec<TenantId>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl Cli {
    /// `--insecure-dev` is the single opt-out from the secure defaults. Returns whether it
    /// switched anything off, so the caller can warn once logging is up.
    pub fn apply_insecure_dev(&mut self) -> bool {
        if !self.insecure_dev {
            return false;
        }
        self.authz_enforce = false;
        self.enforce_image_allowlist = false;
        self.enforce_worker_registry = false;
        true
    }
}
