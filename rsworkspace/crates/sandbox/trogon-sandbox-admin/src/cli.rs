//! The command line: every flag, argument and subcommand, parsed into typed values.

use std::convert::Infallible;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand};
use trogon_sandbox_admin::constants::{
    DEFAULT_ACTIVE_LEASE_BUCKET, DEFAULT_ARTIFACT_TYPE, DEFAULT_IDEMPOTENCY_BUCKET, DEFAULT_NATS_URL,
    DEFAULT_SNAPSHOT_REGISTRY,
};
use trogon_sandbox_admin::quota::parse_month;
use trogon_sandbox_admin::tenant_policy::{validate_host_mount_prefix, validate_pattern};
use trogon_sandbox_admin::{AdminError, parse_artifact_type};
use trogon_sandbox_auth::ApiKeyBucket;
use trogon_sandbox_contract::{
    ArtifactType, CredentialId, CredentialProvider, CredentialProviderError, Digest, LeaseId, ProviderKind,
    ServiceAccountId, SnapshotTagName, TenantId, TokenId, WorkerId,
};
use trogon_sandbox_credential_broker::InProcessKey;
use trogon_sandbox_oci::RegistryHost;
use trogon_sandbox_protocol::subjects::parse_provider_kind;
use trogon_sandbox_substrate_worker::SandboxClass;

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(name = "trogon-sandbox-admin", about = "Operate the sandbox stack")]
pub struct Cli {
    #[arg(long, env = "NATS_URL", default_value = DEFAULT_NATS_URL, global = true)]
    pub nats_url: String,

    #[arg(
        long,
        env = "SANDBOX_API_KEY_BUCKET",
        default_value = trogon_sandbox_auth::DEFAULT_BUCKET,
        global = true,
    )]
    pub api_key_bucket: ApiKeyBucket,

    /// Base64 of the 32-byte key that seals stored credentials. Needed by `credential set`,
    /// `list`, `rotate` and `migrate`.
    #[arg(long, env = "SANDBOX_KMS_KEY", hide_env_values = true, global = true)]
    pub kms_key: Option<InProcessKey>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(subcommand)]
    ApiKey(ApiKeyCommand),
    #[command(subcommand)]
    ImagePolicy(ImagePolicyCommand),
    #[command(subcommand)]
    HostMount(HostMountCommand),
    #[command(subcommand)]
    NativePassthrough(NativePassthroughCommand),
    #[command(subcommand)]
    TenantLimit(TenantLimitCommand),
    #[command(subcommand)]
    Quota(QuotaCommand),
    #[command(subcommand)]
    Lease(LeaseCommand),
    #[command(subcommand)]
    Capability(CapabilityCommand),
    #[command(subcommand)]
    Credential(CredentialCommand),
    #[command(subcommand)]
    Tag(TagCommand),
    #[command(subcommand)]
    Prewarm(PrewarmCommand),
    #[command(subcommand)]
    Snapshot(SnapshotCommand),
    #[command(subcommand)]
    Substrate(SubstrateCommand),
    #[command(subcommand)]
    Worker(WorkerCommand),
}

fn lease_id(raw: &str) -> Result<LeaseId, Infallible> {
    Ok(LeaseId::new(raw))
}

fn credential_id(raw: &str) -> Result<CredentialId, Infallible> {
    Ok(CredentialId::new(raw))
}

fn token_id(raw: &str) -> Result<TokenId, Infallible> {
    Ok(TokenId::new(raw))
}

fn service_account(raw: &str) -> Result<ServiceAccountId, Infallible> {
    Ok(ServiceAccountId::new(raw))
}

fn pattern(raw: &str) -> Result<String, AdminError> {
    validate_pattern(raw)?;
    Ok(raw.to_string())
}

fn host_mount_prefix(raw: &str) -> Result<String, AdminError> {
    validate_host_mount_prefix(raw)?;
    Ok(raw.to_string())
}

/// A credential provider, or `all` for every one of them.
#[derive(Clone, Copy, Debug)]
pub struct ProviderSelection(pub Option<CredentialProvider>);

impl FromStr for ProviderSelection {
    type Err = CredentialProviderError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "all" {
            return Ok(Self(None));
        }
        raw.parse().map(|provider| Self(Some(provider)))
    }
}

#[derive(Subcommand)]
pub enum WorkerCommand {
    /// Stop placing new leases on a worker. Leases it already holds carry on.
    Cordon(WorkerFenceArgs),
    /// Stop placing new leases on a worker and report what is left on it.
    /// Leases finish on their own; migrating them is not automated.
    Drain(WorkerFenceArgs),
    /// Lift a cordon or drain so the worker takes placements again.
    Uncordon(WorkerFenceArgs),
    /// Show a worker's state and the leases still on it.
    Status(WorkerTargetArgs),
    /// List every cordoned or draining worker.
    List,
}

#[derive(Args)]
pub struct WorkerTargetArgs {
    /// Provider token, for example `docker` or `k8s`.
    #[arg(long, value_parser = parse_provider_kind)]
    pub provider: ProviderKind,
    /// The worker's id, as in its heartbeat.
    #[arg(long)]
    pub worker: WorkerId,
    #[arg(long, env = "SANDBOX_ACTIVE_LEASE_BUCKET", default_value = DEFAULT_ACTIVE_LEASE_BUCKET)]
    pub active_lease_bucket: String,
}

#[derive(Args)]
pub struct WorkerFenceArgs {
    #[command(flatten)]
    pub target: WorkerTargetArgs,
    /// Why, shown by `worker list`.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Subcommand)]
pub enum SubstrateCommand {
    /// Build the actor template for an image before any lease asks for it.
    Prebuild(SubstratePrebuildArgs),
}

#[derive(Args)]
pub struct SubstratePrebuildArgs {
    /// The tenant whose atespace the template is built in. Templates are not shared across
    /// tenants, so warming an image for one tenant does nothing for another.
    #[arg(long)]
    pub tenant: TenantId,
    /// The image to warm, pinned by a complete immutable sha256 digest.
    #[arg(long)]
    pub image: String,
    #[arg(long, env = "SANDBOX_SUBSTRATE_API_ENDPOINT")]
    pub ate_api_endpoint: String,
    /// PEM CA certificate that signed the control plane's TLS certificate. Omit for a plaintext
    /// endpoint.
    #[arg(long, env = "SANDBOX_SUBSTRATE_TLS_CA_FILE")]
    pub tls_ca_file: Option<PathBuf>,
    /// TLS server name to check the control plane's certificate against, when it would not
    /// otherwise match the endpoint's host.
    #[arg(long, env = "SANDBOX_SUBSTRATE_TLS_SERVER_NAME")]
    pub tls_server_name: Option<String>,
    /// Bearer token file sent as `authorization: Bearer <token>`. Defaults to the path
    /// Kubernetes projects a pod's own service-account token to.
    #[arg(long, env = "SANDBOX_SUBSTRATE_BEARER_TOKEN_FILE")]
    pub bearer_token_file: Option<PathBuf>,
    #[arg(long, env = "SANDBOX_SUBSTRATE_AGENT_IMAGE")]
    pub agent_image: String,
    #[arg(long, env = "SANDBOX_SUBSTRATE_AGENT_PORT", default_value_t = 8110)]
    pub agent_port: u16,
    #[arg(long, env = "SANDBOX_SUBSTRATE_SANDBOX_CLASS", default_value = "gvisor")]
    pub sandbox_class: SandboxClass,
    /// The `ate.dev/size` label the golden build must land on, matching the label an operator
    /// put on the target `WorkerPool` (for example `small`). The template's own
    /// `worker_selector` is pinned to this, so a prebuild for the wrong size class warms a
    /// snapshot no lease actually landing on that pool class would ever use.
    #[arg(long, env = "SANDBOX_SUBSTRATE_SIZE_CLASS")]
    pub size_class: String,
    #[arg(long, env = "SANDBOX_SUBSTRATE_SNAPSHOTS_LOCATION", default_value = "")]
    pub snapshots_location: String,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub memory_mib: u32,
    #[arg(
        long,
        env = "SANDBOX_SUBSTRATE_ACTOR_CPU_MILLICORES",
        default_value_t = 1000,
        value_parser = clap::value_parser!(u32).range(1..=999_999)
    )]
    pub actor_cpu_millicores: u32,
    #[arg(long, default_value_t = 600)]
    pub timeout_seconds: u64,
}

#[derive(Subcommand)]
pub enum ApiKeyCommand {
    Mint(ApiKeyMintArgs),
    List(ApiKeyListArgs),
    Revoke(ApiKeyRevokeArgs),
}

#[derive(Args)]
pub struct ApiKeyMintArgs {
    #[arg(long)]
    pub tenant: TenantId,
    #[arg(long, value_parser = service_account)]
    pub service_account: ServiceAccountId,
    #[arg(long)]
    pub name: String,
}

#[derive(Args)]
pub struct ApiKeyListArgs {
    #[arg(long)]
    pub tenant: Option<TenantId>,
}

#[derive(Args)]
pub struct ApiKeyRevokeArgs {
    /// The raw token or its stored hash.
    #[arg(long)]
    pub token: String,
}

#[derive(Subcommand)]
pub enum ImagePolicyCommand {
    Set(ImagePolicySetArgs),
    Show(TenantArgs),
    Clear(TenantArgs),
}

#[derive(Args)]
pub struct TenantArgs {
    #[arg(long)]
    pub tenant: TenantId,
}

#[derive(Args)]
pub struct ImagePolicySetArgs {
    #[arg(long)]
    pub tenant: TenantId,
    #[arg(long = "pattern", num_args = 1.., value_parser = pattern)]
    pub patterns: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub replace: bool,
}

#[derive(Subcommand)]
pub enum HostMountCommand {
    Set(HostMountSetArgs),
    Show(TenantArgs),
    Clear(TenantArgs),
}

#[derive(Args)]
pub struct HostMountSetArgs {
    #[arg(long)]
    pub tenant: TenantId,
    #[arg(long = "prefix", num_args = 1.., value_parser = host_mount_prefix)]
    pub prefixes: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub replace: bool,
}

#[derive(Subcommand)]
pub enum NativePassthroughCommand {
    Set(NativePassthroughSetArgs),
    Show(TenantArgs),
    Clear(TenantArgs),
}

#[derive(Args)]
pub struct NativePassthroughSetArgs {
    #[arg(long)]
    pub tenant: TenantId,
    /// Record an explicit deny instead of an allow. A tenant on the broker's static list stays
    /// allowed either way.
    #[arg(long, default_value_t = false)]
    pub deny: bool,
}

#[derive(Subcommand)]
pub enum TenantLimitCommand {
    Set(TenantLimitSetArgs),
    Show(TenantArgs),
    Clear(TenantArgs),
}

#[derive(Args)]
pub struct TenantLimitSetArgs {
    #[arg(long)]
    pub tenant: TenantId,
    #[arg(long)]
    pub max_cpu_seconds: u32,
    #[arg(long)]
    pub max_memory_mib: u32,
    /// Tag key to cap per value, such as `user`. Needs --per-tag-max.
    #[arg(long, requires = "per_tag_max")]
    pub per_tag_key: Option<String>,
    /// Most live leases one value of --per-tag-key may hold.
    #[arg(long, requires = "per_tag_key")]
    pub per_tag_max: Option<u32>,
}

#[derive(Subcommand)]
pub enum PrewarmCommand {
    Set(PrewarmSetArgs),
    Show(PrewarmShowArgs),
    Clear(PrewarmClearArgs),
}

#[derive(Args)]
pub struct PrewarmSetArgs {
    pub tenant: TenantId,
    pub digest: Digest,
    pub size: u32,
    #[arg(num_args = 1.., value_parser = parse_provider_kind)]
    pub providers: Vec<ProviderKind>,
}

#[derive(Args)]
pub struct PrewarmShowArgs {
    pub tenant: TenantId,
}

#[derive(Args)]
pub struct PrewarmClearArgs {
    pub tenant: TenantId,
    pub digest: Digest,
}

#[derive(Subcommand)]
pub enum TagCommand {
    Set(TagSetArgs),
    Get(TagNameArgs),
    List(TagListArgs),
    History(TagNameArgs),
    Clear(TagNameArgs),
}

#[derive(Args)]
pub struct TagSetArgs {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
    pub digest: Digest,
    #[arg(long, default_value = DEFAULT_ARTIFACT_TYPE, value_parser = parse_artifact_type)]
    pub artifact_type: ArtifactType,
    #[arg(long)]
    pub expected_revision: Option<u64>,
}

#[derive(Args)]
pub struct TagNameArgs {
    pub tenant: TenantId,
    pub name: SnapshotTagName,
}

#[derive(Args)]
pub struct TagListArgs {
    pub tenant: TenantId,
}

#[derive(Subcommand)]
pub enum SnapshotCommand {
    /// Delete a snapshot artifact by digest directly from the OCI registry.
    ///
    /// Unlike `tag clear`, which only removes the KV tag pointer, this removes the underlying
    /// artifact itself, regardless of whether a tag still references it.
    Delete(SnapshotDeleteArgs),
    /// Look up whether a lease's snapshot capture succeeded or failed.
    Status(SnapshotStatusArgs),
}

#[derive(Args)]
pub struct SnapshotStatusArgs {
    #[arg(value_parser = lease_id)]
    pub lease_id: LeaseId,
}

#[derive(Args)]
pub struct SnapshotDeleteArgs {
    pub tenant: TenantId,
    pub digest: Digest,
    #[arg(long, default_value = DEFAULT_ARTIFACT_TYPE, value_parser = parse_artifact_type)]
    pub artifact_type: ArtifactType,
    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = DEFAULT_SNAPSHOT_REGISTRY)]
    pub registry: RegistryHost,
    #[arg(long, default_value_t = false)]
    pub insecure_registry: bool,
}

#[derive(Subcommand)]
pub enum CredentialCommand {
    Set(CredentialSetArgs),
    List(CredentialListArgs),
    Rotate(CredentialRotateArgs),
    RevokeToken(CredentialRevokeTokenArgs),
    /// Rewrite legacy (no-AAD) credential records in the envelope format.
    Migrate(CredentialMigrateArgs),
}

#[derive(Args)]
pub struct CredentialSetArgs {
    /// Tenant that owns the credential; bound into its encryption.
    #[arg(long)]
    pub tenant: TenantId,
    pub provider: CredentialProvider,
    #[arg(value_parser = credential_id)]
    pub credential_id: CredentialId,
}

#[derive(Args)]
pub struct CredentialListArgs {
    pub provider: CredentialProvider,
}

#[derive(Args)]
pub struct CredentialRotateArgs {
    /// Tenant that owns the credential.
    #[arg(long)]
    pub tenant: TenantId,
    pub provider: CredentialProvider,
    #[arg(value_parser = credential_id)]
    pub credential_id: CredentialId,
}

#[derive(Args)]
pub struct CredentialRevokeTokenArgs {
    #[arg(long)]
    pub tenant: TenantId,
    pub provider: CredentialProvider,
    #[arg(value_parser = token_id)]
    pub token_id: TokenId,
}

#[derive(Args)]
pub struct CredentialMigrateArgs {
    /// Tenant that owns the legacy records being migrated.
    #[arg(long)]
    pub tenant: TenantId,
    /// Report what would be migrated without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Migrate only this legacy record. Repeat it to name several. A legacy record names no
    /// tenant, so when several tenants own legacy records, run the command once per tenant with
    /// that tenant's ids.
    #[arg(long = "credential-id", value_parser = credential_id)]
    pub credential_ids: Vec<CredentialId>,
    /// A provider name, or `all`.
    pub provider: ProviderSelection,
}

#[derive(Subcommand)]
pub enum LeaseCommand {
    /// Drop the marker of a pause or resume the broker gave up settling, accepting the worker's
    /// state as it is.
    ClearFreeze(LeaseFreezeArgs),
    /// Reset the marker's attempt count so the broker asks the worker again.
    RetryFreeze(LeaseFreezeArgs),
}

#[derive(Args)]
pub struct LeaseFreezeArgs {
    #[arg(value_parser = lease_id)]
    pub lease_id: LeaseId,
    /// Act on a marker the broker is still working, not only one it gave up on (listed by the
    /// `sandbox_broker_unsettled_freezes` gauge).
    #[arg(long)]
    pub force: bool,
    #[arg(long, env = "SANDBOX_ACTIVE_LEASE_BUCKET", default_value = DEFAULT_ACTIVE_LEASE_BUCKET)]
    pub active_lease_bucket: String,
    #[arg(long, env = "SANDBOX_IDEMPOTENCY_BUCKET", default_value = DEFAULT_IDEMPOTENCY_BUCKET)]
    pub idempotency_bucket: String,
}

#[derive(Subcommand)]
pub enum QuotaCommand {
    Show(QuotaShowArgs),
}

#[derive(Args)]
pub struct QuotaShowArgs {
    #[arg(long)]
    pub tenant: TenantId,
    #[arg(long, value_name = "YYYY-MM", value_parser = parse_month)]
    pub month: Option<(i32, u32)>,
}

#[derive(Subcommand)]
pub enum CapabilityCommand {
    Show(CapabilityShowArgs),
    Diff(CapabilityDiffArgs),
}

#[derive(Args)]
pub struct CapabilityShowArgs {
    #[arg(value_parser = parse_provider_kind)]
    pub provider: ProviderKind,
}

#[derive(Args)]
pub struct CapabilityDiffArgs {
    #[arg(value_parser = parse_provider_kind)]
    pub provider_a: ProviderKind,
    #[arg(value_parser = parse_provider_kind)]
    pub provider_b: ProviderKind,
}
