use std::time::Duration;

pub const WORKSPACE_PATH: &str = "/workspace";
pub const EVIDENCE_PATH: &str = "/evidence";
pub const RUNTIME_TMP_VOLUME: &str = "runtime-tmp";
pub const RUNTIME_TMP_MAX_MIB: u32 = 8;
pub const GIT_INIT_IMAGE: &str = "cgr.dev/chainguard/git:latest";
pub const WORKSPACE_IMPORT_INIT_IMAGE: &str = "busybox:1.36.1";
pub const WORKSPACE_TAR_KEY: &str = "workspace.tar";

/// Pod-level seccomp profile applied to every sandbox pod.
///
/// The RuntimeClass is the real boundary for the microVM/gVisor tiers, but
/// `IsolationTier::Container` resolves to plain runc and an operator-configured
/// RuntimeClass can silently do the same (which is why
/// `is_strong_isolation_runtime_class`
/// exists). The syscall filter is therefore unconditional rather than a
/// property of the tier.
pub const SECCOMP_PROFILE_TYPE: &str = "RuntimeDefault";

/// Uid and gid the agent runs as when the lease names no user. The agent
/// container must run as non-root, and Kubernetes refuses to start an image
/// whose `USER` is root (plain alpine, debian, ...) under that rule unless a
/// uid is given. 65532 is the conventional unprivileged "nonroot" id.
pub const DEFAULT_AGENT_UID: i64 = 65532;

/// Group that owns the workspace volume, so the agent can write it whatever
/// uid it runs as. The init containers that fill the workspace run as root
/// and hand group write to this group.
pub const WORKSPACE_FS_GROUP: i64 = DEFAULT_AGENT_UID;

/// Keeps the agent container alive for exec. `sleep infinity` as PID 1 has
/// no SIGTERM handler, so the kernel ignores the signal and every pod delete
/// waits out the full grace period; this loop exits on the first TERM. QUIT
/// is trapped too because the kubelet sends an image's `STOPSIGNAL` instead
/// (nginx images declare SIGQUIT).
pub const KEEP_ALIVE_SCRIPT: &str = "trap 'exit 0' TERM INT QUIT; while :; do sleep 3600 & wait $!; done";

/// Bounds how long a sandbox pod may take to stop. Nothing in the pod holds
/// state worth flushing on shutdown, and the default 30s turns every lease
/// release into a 30s wait when a process ignores SIGTERM.
pub const SANDBOX_TERMINATION_GRACE_SECONDS: i64 = 5;

pub const LABEL_LEASE_ID: &str = "sandbox.trogon.live/lease-id";
pub const LABEL_TENANT_ID: &str = "sandbox.trogon.live/tenant-id";
/// The stable identity of the worker replica that launched or claimed a lease
/// resource, so replicas sharing a namespace recover only their own leases.
pub const LABEL_WORKER_IDENTITY: &str = "sandbox.trogon.live/worker-identity";
pub const LABEL_DIGEST: &str = "sandbox.trogon.live/digest";
/// The full digest of a warm pod, which its `LABEL_DIGEST` label cannot hold.
pub const ANNOTATION_DIGEST: &str = "sandbox.trogon.live/digest";
/// Where a warm pod's snapshot was pulled from, so a restarted worker pulls
/// the next warm pod for the same digest from the same place.
pub const ANNOTATION_SOURCE_REGISTRY: &str = "sandbox.trogon.live/source-registry";
pub const ANNOTATION_SOURCE_REPOSITORY: &str = "sandbox.trogon.live/source-repository";
pub const LABEL_WARM_POOL: &str = "sandbox.trogon.live/warm-pool";
pub const WARM_POOL_IDLE: &str = "idle";
pub const WARM_POOL_CLAIMED: &str = "claimed";
/// When a claimed warm pod's lease runs out, as Unix seconds on the apiserver
/// clock. A warm pod has no Job to carry `activeDeadlineSeconds`, so the claim
/// stamps the deadline here and the sweeper reads it back.
pub const LABEL_LEASE_DEADLINE: &str = "sandbox.trogon.live/lease-deadline";

/// How long past its deadline another replica's lease resource must be before
/// this replica reclaims it. A live owner sweeps every minute, so a resource
/// still here this long after its deadline belongs to a replica that is dead or
/// stuck; the margin keeps a live owner's table from being pruned under it.
pub const FOREIGN_LEASE_GRACE_SECONDS: i64 = 600;

/// How old a lease resource with no deadline must be before another replica
/// reclaims it as an orphan: its owner's pod is gone from the namespace (or
/// nothing is configured to adopt an unlabeled resource), and a no-deadline
/// lease leaves no other signal. The margin covers the owner being briefly
/// absent while its pod restarts or reschedules, since the pod that comes back
/// under the same name recovers its own leases. Zero disables the reclaim.
pub const ORPHAN_RECLAIM_GRACE_SECONDS: i64 = 3600;

/// Key of the serialized lease request inside the Secret that
/// [`lease_request_secret_name`](crate::k8s_ops::lease_request_secret_name) names.
pub const LEASE_REQUEST_KEY: &str = "lease-request.json";

/// The pod volume that carries a restore Secret into the import init container.
pub const RESTORE_VOLUME_NAME: &str = "restore-data";

/// Name of the container that runs the sandboxed agent.
pub const AGENT_CONTAINER_NAME: &str = "agent";

/// Container waiting reasons a pod does not recover from by itself.
pub const FATAL_WAITING_REASONS: &[&str] = &[
    "CreateContainerConfigError",
    "CreateContainerError",
    "ErrImagePull",
    "ImagePullBackOff",
    "InvalidImageName",
    "ErrImageNeverPull",
    "CrashLoopBackOff",
    "RunContainerError",
];

pub const JOB_POD_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How long a launch waits for the registry to answer an image `USER` lookup
/// before it falls back to the default uid.
pub const IMAGE_USER_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a successful image `USER` lookup is reused.
pub const IMAGE_USER_CACHE_TTL: Duration = Duration::from_secs(600);

/// How many bytes a pod-exec reader pulls per read.
pub const EXEC_READ_CHUNK_BYTES: usize = 8192;

/// How many resize requests may queue ahead of the terminal.
pub const TTY_RESIZE_QUEUE: usize = 8;
