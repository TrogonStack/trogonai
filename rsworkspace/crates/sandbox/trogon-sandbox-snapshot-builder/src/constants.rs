use std::time::Duration;

pub(crate) const RELEASE_QUEUE_GROUP: &str = "trogon-sandbox-snapshot-builders";
pub(crate) const TAG_RECOVERY_CONSUMER: &str = "trogon-sandbox-snapshot-tag-recovery";
pub(crate) const TAG_RECOVERY_RETRY_DELAY: Duration = Duration::from_secs(1);
pub(crate) const TAG_RECOVERY_ACK_WAIT: Duration = Duration::from_secs(30);
pub(crate) const TAG_RECOVERY_MAX_ACK_PENDING: i64 = 64;

pub(crate) const MAX_EXPORT_ATTEMPTS: u32 = 5;
pub(crate) const INITIAL_EXPORT_BACKOFF: Duration = Duration::from_millis(250);

pub(crate) const MAX_TAG_CAS_ATTEMPTS: usize = 5;
pub(crate) const TAG_AUDIT_JETSTREAM_REPLICAS: usize = 1;
pub(crate) const TAG_SET_BY: &str = "trogon-sandbox-snapshot-builder";

/// Delays between retries of the tag step alone. The step is idempotent (the same digest is a no-op and a stale
/// release is skipped), so re-running it never rebuilds or republishes the snapshot. Bounded: at most
/// `TAG_RETRY_BACKOFF.len() + 1` attempts per release.
pub(crate) const TAG_RETRY_BACKOFF: &[Duration] = &[
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(4),
];

pub(crate) const KV_CAS_ATTEMPTS: usize = 16;
pub(crate) const ACCOUNTING_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const RELEASE_CHANNEL_CAPACITY: usize = 256;

pub(crate) const EXTRACT_FLAG: &str = "--internal-firecracker-scan-extract";
pub(crate) const EXTRACT_ARGUMENT_COUNT: usize = 5;
pub(crate) const FIRECRACKER_LAYER_ROLE: &str = "io.trogon.sandbox.firecracker.file";
pub(crate) const FIRECRACKER_LAYER_ROLES: [&str; 4] =
    ["snapshot.json", "snapshot.state", "snapshot.memory", "rootfs.ext4"];
pub(crate) const FIRECRACKER_LAYER_COUNT: usize = FIRECRACKER_LAYER_ROLES.len();
pub(crate) const METADATA_MAX_BYTES: usize = 64 * 1024;
pub(crate) const EXTRACT_JOB_MAX_BYTES: u64 = 4096;
pub(crate) const STDERR_MAX_BYTES: u64 = 64 * 1024;
pub(crate) const COPY_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const PATH_MAX_BYTES: usize = 4096;
pub(crate) const EXTRACTOR_ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const EXTRACTOR_OPEN_FILES: u64 = 64;
pub(crate) const EXTRACTOR_POLL_INTERVAL: Duration = Duration::from_millis(5);
pub(crate) const SNAPSHOT_FORMAT_VERSION: u64 = 1;

pub(crate) const MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 250_000;
pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const EXTRACT_TIMEOUT_MS: u64 = 120_000;

pub(crate) const SBOM_MEDIA_TYPE: &str = "application/json";
pub(crate) const SCAN_LAYER_FILE: &str = "snapshot-layer.tar";

pub(crate) const SPAN_SNAPSHOT_EXPORT: &str = "snapshot_export";
pub(crate) const SPAN_SNAPSHOT_SECURITY_PIPELINE: &str = "snapshot_security_pipeline";
pub(crate) const SPAN_SNAPSHOT_KV_WRITE: &str = "snapshot_kv_write";
pub(crate) const SPAN_SNAPSHOT_PUBLISHED: &str = "snapshot_published";
