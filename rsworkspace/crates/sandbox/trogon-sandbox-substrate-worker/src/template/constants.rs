use std::time::Duration;

/// JetStream KV bucket indexing which template versions exist and what became of their
/// golden build.
pub const TEMPLATE_KV_BUCKET: &str = "sandbox-substrate-template";

/// Hex characters of the workload digest kept in a template name.
pub(super) const TEMPLATE_KEY_HEX_LEN: usize = 32;

/// How long a recorded build failure is trusted before another lease is allowed to retry.
///
/// Short on purpose: a failure is usually a bad image, and every lease paying the full build
/// timeout to rediscover that is the thing this cache prevents. But a failure can also be a
/// transient cluster fault, and permanently poisoning a template on one bad afternoon would
/// be worse than an occasional wasted build.
pub(super) const FAILURE_TTL: Duration = Duration::from_secs(300);

/// How often a lease re-reads a template whose golden snapshot is still building.
pub(super) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The smallest memory a microvm actor can boot with.
pub(super) const MICROVM_MIN_MEMORY_MIB: u32 = 128;

/// The volume layout baked into a template.
///
/// A `durableDir` is the only volume primitive usable here: `externalVolumeTemplate` is CSI,
/// gVisor-only, and dies with the actor, which is not our drive semantics.
pub(super) const WORKSPACE_VOLUME: &str = "workspace";
pub(super) const WORKSPACE_MOUNT_PATH: &str = "/workspace";
pub(super) const AGENT_CONTAINER: &str = "sandbox-actor-agent";

/// Where the agent binary lives inside the tenant's image.
///
/// The agent is copied in by the image build, so this path is a contract between this worker
/// and whatever produced `--agent-image`.
pub(super) const AGENT_BINARY_PATH: &str = "/usr/local/bin/sandbox-actor-agent";
