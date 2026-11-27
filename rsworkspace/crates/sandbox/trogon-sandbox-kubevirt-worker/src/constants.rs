use std::time::Duration;

/// Label carrying the lease a resource belongs to.
pub const LEASE_LABEL: &str = "sandbox.trogon.ai/lease-id";

/// Label carrying the tenant that owns a resource.
pub const TENANT_LABEL: &str = "sandbox.trogon.ai/tenant-id";

/// Label carrying the worker that created a resource.
pub const WORKER_LABEL: &str = "sandbox.trogon.ai/worker-id";

/// Label the guest network policy selects. It sits on the VMI template so the virt-launcher pod carries it.
pub const GUEST_POLICY_LABEL: &str = "sandbox.trogonai.io/lease-agent";

/// Label marking a VM as part of the warm pool.
pub const WARM_POOL_LABEL: &str = "sandbox.trogon.ai/warm-pool";

/// Value of [`WARM_POOL_LABEL`] for a VM nobody has claimed.
pub const WARM_POOL_IDLE: &str = "idle";

/// Label naming the warm pool slot a VM fills.
pub const WARM_POOL_SLOT_LABEL: &str = "sandbox.trogon.ai/warm-slot";

/// Annotation carrying the snapshot digest a warm VM was restored from.
pub const DIGEST_ANNOTATION: &str = "sandbox.trogon.ai/digest";

/// Annotation naming the root data volume of a warm VM.
pub const ROOT_DV_ANNOTATION: &str = "sandbox.trogon.ai/root-dv";

/// Annotation carrying the uid of the root data volume of a warm VM.
pub const ROOT_DV_UID_ANNOTATION: &str = "sandbox.trogon.ai/root-dv-uid";

/// Annotation carrying the token of the claim in flight on a warm VM.
pub const CLAIM_ANNOTATION: &str = "sandbox.trogon.ai/claim-id";

/// Annotation carrying the allocation a VM serves.
pub const ALLOCATION_ANNOTATION: &str = "sandbox.trogon.ai/allocation-id";

/// Annotation carrying the guest memory a warm VM booted with.
pub const WARM_MEMORY_ANNOTATION: &str = "sandbox.trogon.ai/warm-memory-mib";

/// Longest Kubernetes DNS-1123 label.
pub const MAX_RESOURCE_NAME_LEN: usize = 63;

/// Version of the durable custody record format.
pub const CUSTODY_VERSION: u32 = 1;

/// Most bytes a custody file may hold.
pub const CUSTODY_MAX_BYTES: u64 = 64 * 1024;

/// Most lease records one custody directory may hold.
pub const CUSTODY_MAX_RECORDS: usize = 4096;

/// Longest custody name: a namespace, a resource name, or a uid.
pub const CUSTODY_MAX_NAME_LEN: usize = 253;

/// Longest lease identity a custody file name may carry.
pub const CUSTODY_MAX_LEASE_LEN: usize = 128;

/// Longest resource version a warm claim may carry.
pub const CUSTODY_MAX_RESOURCE_VERSION_LEN: usize = 256;

/// Unix mode of custody files.
#[cfg(unix)]
pub const CUSTODY_FILE_MODE: u32 = 0o600;

/// Unix mode of the custody directory.
#[cfg(unix)]
pub const CUSTODY_DIR_MODE: u32 = 0o700;

/// The queue group that spreads KubeVirt lease dispatches across workers.
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-kubevirt";

/// Program the guest exec path runs for `virtctl` unless configured otherwise.
pub const DEFAULT_VIRTCTL: &str = "virtctl";

/// Program the guest exec path falls back to when `virtctl` is unavailable, unless configured otherwise.
pub const DEFAULT_KUBECTL: &str = "kubectl";

/// The guest account the ephemeral SSH keys are installed for.
pub const GUEST_USER: &str = "sandbox";

/// The port the guest's `sshd` listens on.
pub const GUEST_SSH_PORT: u16 = 22;

/// Most leases a worker keeps a guest SSH session for. Sessions exist only for leases the worker holds, so this is a
/// backstop, not a working limit.
pub const MAX_GUEST_SESSIONS: usize = 256;

/// How long a connection to a guest's `sshd`, authentication included, may take.
pub const GUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long releasing a lease waits for its guest SSH session to say goodbye.
pub const GUEST_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The smallest CPU request a lease may name, in thousandths of a core.
pub const MIN_VCPU_MILLIS: u32 = 100;

/// The largest CPU request a lease may name, in thousandths of a core. A VM asking for more than a node has stays
/// unscheduled until the lease times out.
pub const MAX_VCPU_MILLIS: u32 = 8000;

/// Median time to boot a VM from a registry image, in milliseconds.
pub const COLD_START_P50_MS: u32 = 5000;

/// Median time to boot a VM from a restored snapshot, in milliseconds.
pub const COLD_RESTORE_P50_MS: u32 = 30000;

/// Most warm VMs the profile advertises.
pub const PROFILE_MAX_WARM_POOL_SIZE: u32 = 4;

/// Most sandboxes the profile advertises running at once.
pub const PROFILE_MAX_CONCURRENT_SANDBOXES: u32 = 2;

/// Value of [`TENANT_LABEL`] on a warm VM nobody has claimed.
pub const WARM_POOL_TENANT: &str = "warm-pool";

/// Value of [`LEASE_LABEL`] on a warm VM nobody has claimed.
pub const WARM_POOL_UNASSIGNED_LEASE: &str = "unassigned";

/// Guest memory of a warm VM created before [`WARM_MEMORY_ANNOTATION`] existed. Those VMs were sized from the
/// lease that prewarmed them with a 512 MiB floor, which is also the default warm size.
pub const LEGACY_WARM_MEMORY_MIB: u32 = 512;

/// Label KubeVirt puts on a VM snapshot content naming the snapshot it belongs to.
pub const SNAPSHOT_SOURCE_LABEL: &str = "snapshot.kubevirt.io/source-vm-snapshot-name";

/// Image of the pod that streams a snapshot PVC's files to its log.
pub const EXPORT_POD_IMAGE: &str = "quay.io/curl/curl:8.5.0";

/// Longest wait for a DataVolume import, a VMI to start or stop, or a VM snapshot to become ready.
pub const KUBEVIRT_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Longest wait for the PVC export pod to finish.
pub const EXPORT_POD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Pause between two reads of a resource being waited on.
pub const KUBEVIRT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// The smallest guest memory a cold-started VM boots with, in MiB.
pub const MIN_COLD_MEMORY_MIB: u32 = 512;

/// The tenant label of resources a snapshot export creates.
pub const SNAPSHOT_TENANT: &str = "snapshot";

/// How often the worker reconciles the warm pool with the cluster and evicts idle VMs.
pub const WARM_POOL_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// How long in-flight requests may keep running after shutdown starts before they are abandoned.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

/// The digest a checkpoint target carries until the push assigns the real one.
pub const CHECKPOINT_PLACEHOLDER_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Environment variable holding the access key id of the restore staging store.
pub const AWS_ACCESS_KEY_ID_ENV: &str = "AWS_ACCESS_KEY_ID";

/// Environment variable holding the secret access key of the restore staging store.
pub const AWS_SECRET_ACCESS_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";

/// Environment variable holding the session token of the restore staging store.
pub const AWS_SESSION_TOKEN_ENV: &str = "AWS_SESSION_TOKEN";
