/// Longest wall-clock lifetime, in seconds, a lease may be created or extended to unless the
/// operator raises the cap.
pub const MAX_LEASE_TTL_SECONDS: u32 = 86_400;

/// Paused budget applied when a pause request names none, in seconds.
pub const DEFAULT_MAX_PAUSED_SECONDS: u32 = 3600;
/// Longest paused budget a caller may ask for, in seconds; larger requests are clamped.
pub const MAX_PAUSED_SECONDS_CEILING: u32 = 7 * 24 * 3600;
/// Suspended budget applied when a suspend request names none, in seconds.
pub const DEFAULT_MAX_SUSPENDED_SECONDS: u32 = 24 * 3600;
/// Longest suspended budget a caller may ask for, in seconds; larger requests are clamped.
pub const MAX_SUSPENDED_SECONDS_CEILING: u32 = 7 * 24 * 3600;
/// Consecutive failed wakes after which a suspended lease is given up on.
pub const MAX_WAKE_FAILURES: u32 = 3;

/// Longest a chunked file transfer request waits for its reply.
pub const FILE_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a worker may take to answer a command readiness query.
pub const COMMAND_READINESS_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Largest command readiness reply the broker reads, in bytes.
pub const COMMAND_READINESS_MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// CPU seconds a prewarm lease is sized with.
pub const PREWARM_CPU_SECONDS: std::num::NonZeroU32 = match std::num::NonZeroU32::new(60) {
    Some(value) => value,
    None => std::num::NonZeroU32::MIN,
};
/// Memory, in MiB, a prewarm lease is sized with.
pub const PREWARM_MEMORY_MIB: std::num::NonZeroU32 = match std::num::NonZeroU32::new(256) {
    Some(value) => value,
    None => std::num::NonZeroU32::MIN,
};
/// Lifetime, in seconds, of a prewarm lease.
pub const PREWARM_LEASE_TTL_SECONDS: u32 = 3600;

/// Page size a lease listing uses when the request names none.
pub const DEFAULT_LIST_LEASES_LIMIT: u32 = 50;
/// Largest page a lease listing returns.
pub const MAX_LIST_LEASES_LIMIT: u32 = 200;

/// Worker id the always-available registry reports for every provider.
pub const ALWAYS_AVAILABLE_WORKER: &str = "always-available";

/// Drive size applied when a create request names none.
pub(crate) const DEFAULT_DRIVE_SIZE_GIB: u64 = 100;

/// Largest drive size a create request may name.
pub(crate) const MAX_DRIVE_SIZE_GIB: u64 = 1024;

/// Environment variable that asks a worker for a persistent runtime when set to `true` or `1`.
pub const PERSISTENT_RUNTIME_ENV: &str = "V8_WASM_PERSISTENT_RUNTIME";

/// Largest memory, in MiB, a substrate size class offers (the `large` class).
pub const SUBSTRATE_MAX_MEMORY_MIB: u32 = 32_768;

/// NATS header carrying the caller's bearer token.
pub const AUTHORIZATION_HEADER: &str = "Authorization";
/// Scheme prefix of the bearer token value.
pub const BEARER_PREFIX: &str = "Bearer ";
/// Principal recorded when authentication is switched off.
pub const UNAUTHENTICATED_PRINCIPAL: &str = "unauthenticated";

/// How long a per-tenant policy record is kept in its KV bucket, so a tenant that went quiet
/// keeps its policy for a year.
pub const TENANT_RECORD_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24 * 365);

/// Attempts a compare-and-swap entity write makes before giving up.
pub(crate) const MAX_CAS_RETRIES: u8 = 8;

/// Revisions the entity key-value buckets keep per key.
pub(crate) const ENTITY_KV_HISTORY: i64 = 1;

/// Longest entity name, in bytes.
pub(crate) const MAX_ENTITY_NAME_BYTES: usize = 128;

/// Stream holding command output. JetStream stream names may not contain `.`.
pub const COMMAND_LOG_STREAM: &str = "sandbox_command_logs";

/// Subject filter the stream captures: every execution's output chunks.
pub const COMMAND_LOG_SUBJECT_FILTER: &str = "sandbox.evidence.*.command.*";

/// Stream holding one exit record per command, sourced from
/// [`COMMAND_LOG_STREAM`] so it can live longer than the output chunks.
pub const COMMAND_EXIT_STREAM: &str = "sandbox_command_exits";

/// Subject filter for every execution's exit marker, captured by the same stream.
pub const COMMAND_EXIT_SUBJECT_FILTER: &str = "sandbox.evidence.*.command_exit.*";

/// Most chunks one page reads, so a page of tiny chunks cannot turn into an
/// unbounded number of lookups. A page cut here reports `has_more`.
pub const MAX_CHUNKS_PER_PAGE: usize = 1024;

/// A worker may still be flushing a command's last chunks when the lease ends
/// and the broker purges, so a chunk can land after the purge and outlive it.
/// A second purge this long afterwards removes such stragglers.
pub(crate) const LATE_CHUNK_SWEEP: std::time::Duration = std::time::Duration::from_secs(30);

/// Events kept per entity subject.
pub(crate) const ENTITY_EVENTS_PER_SUBJECT: i64 = 1000;

/// How long an entity event stays in the stream.
pub(crate) const ENTITY_EVENT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24 * 365);

/// Largest cancellation authority record a launch custody store accepts.
pub(crate) const MAX_CANCELLATION_AUTHORITY_BYTES: usize = 16 * 1024;

/// Reply-journal retention, independent of nonexpiring cancellation custody.
/// Both stores expire replies on this interval; custody continues to refuse
/// another effect after reply expiry or explicit reply cleanup.
pub const LAUNCH_KEY_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(25 * 60 * 60);

/// TTL for abandoned prewarm KV entries (auto-prune).
pub const PREWARM_KV_TTL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Controller tick interval.
pub const PREWARM_TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a released lease's final figures stay readable after the last
/// write to them.
pub const USAGE_RETENTION: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// How long a quota record and its attempt receipts are retained.
pub const QUOTA_RETENTION: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24 * 95);

/// Active lease cap a quota record starts with when the operator names none.
pub const DEFAULT_ACTIVE_LEASE_CAP: u32 = 50;

/// CPU seconds a quota record created on demand is capped at.
pub const DEFAULT_QUOTA_CPU_SECONDS: u32 = 3600;

/// Wall seconds a quota record created on demand is capped at.
pub const DEFAULT_QUOTA_WALL_SECONDS: u32 = 7200;

/// Snapshot count a tenant may hold unless the operator names another cap.
pub const DEFAULT_SNAPSHOT_COUNT_CAP: u64 = 100;

/// Snapshot bytes a tenant may hold unless the operator names another cap.
pub const DEFAULT_SNAPSHOT_BYTES_CAP: u64 = 100 * 1024 * 1024 * 1024;

/// Concurrent prewarm leases a tenant may hold by default.
pub const DEFAULT_PREWARM_COUNT_CAP: u32 = 20;

/// Prewarm CPU seconds a tenant may hold by default.
pub const DEFAULT_PREWARM_CPU_CAP: u64 = 3600;

/// Most quota keys the legacy snapshot inventory scan visits before it refuses.
pub const LEGACY_SNAPSHOT_INVENTORY_BOUND: usize = 65_536;

/// How far in the future an attempt creation time may be and still count as clock skew.
pub const ATTEMPT_CLOCK_SKEW_TOLERANCE: std::time::Duration = std::time::Duration::from_secs(300);

/// How long a refund receipt is kept. Longer than the pending-purge marker's
/// lifetime, the only thing that can ask for the same refund again.
pub const REFUND_RECEIPT_RETENTION: std::time::Duration = std::time::Duration::from_secs(8 * 24 * 60 * 60);

/// Most receipts one quota record keeps. Bounds the record well under the KV
/// value size limit for a tenant that deletes many leases within the
/// retention window; past it the oldest receipts go first, so only a redo of
/// a very old purge could refund twice.
pub const MAX_REFUND_RECEIPTS: usize = 4096;

/// Most bytes of receipts one quota record keeps, counting each lease id plus
/// [`REFUND_RECEIPT_OVERHEAD_BYTES`]. The count cap alone leaves the size to
/// the length of lease ids, which a caller can choose; this keeps the record
/// at about half the default 1 MiB NATS payload limit whatever they are.
pub const MAX_REFUND_RECEIPT_BYTES: usize = 384 * 1024;

/// What one receipt costs beyond its lease id: quotes, colon, comma and a
/// Unix second.
pub const REFUND_RECEIPT_OVERHEAD_BYTES: usize = 24;

/// How long a granted reply is replayed for its idempotency key. The KV
/// bucket ages entries out on it; the in-memory store expires them the same
/// way so it does not hold every key for the life of the process.
pub const IDEMPOTENCY_RECORD_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Longest tenant id a live snapshot projection row may carry.
pub(crate) const SNAPSHOT_LIVE_MAX_TENANT_BYTES: usize = 256;

/// Compare-and-swap attempts before a live snapshot projection write gives up.
pub(crate) const SNAPSHOT_LIVE_CAS_RETRIES: usize = 16;

/// Capacity of the live snapshot projection bucket.
pub(crate) const SNAPSHOT_LIVE_BUCKET_MAX_BYTES: i64 = 64 * 1024 * 1024;

/// Largest value a live snapshot projection row may hold.
pub(crate) const SNAPSHOT_LIVE_MAX_VALUE_BYTES: i32 = 8 * 1024;

/// Compare-and-swap attempts before a snapshot tag write gives up.
pub(crate) const SNAPSHOT_TAG_MAX_CAS_RETRIES: u8 = 8;

/// Revisions the snapshot tag bucket keeps per key.
pub(crate) const SNAPSHOT_TAG_KV_HISTORY: i64 = 64;

/// How long snapshot tag records and their audit events are retained.
pub(crate) const SNAPSHOT_TAG_RETENTION: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24 * 365);

/// Most audit events one tag history read returns.
pub(crate) const SNAPSHOT_TAG_AUDIT_FETCH_LIMIT: usize = 10_000;

/// Name of the work-queue stream that carries snapshot publication intents.
pub const SNAPSHOT_PUBLICATION_STREAM: &str = "sandbox-snapshot-tag-recovery";

/// Most capture or deletion receipts one tenant's accounting row holds.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_RECEIPTS: usize = 512;

/// Largest encoded tenant accounting row.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_RECORD_BYTES: usize = 768 * 1024;

/// Compare-and-swap attempts before an accounting mutation gives up.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_CAS_ATTEMPTS: usize = 16;

/// Longest tenant id, capture key or lease id the accounting row accepts.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_KEY_BYTES: usize = 256;

/// Longest registry or repository name a capture binding accepts.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_REFERENCE_BYTES: usize = 1024;

/// Largest publication intent the accounting store stages.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_INTENT_BYTES: usize = 64 * 1024;

/// Most tenants one publication recovery pass scans.
pub(crate) const SNAPSHOT_ACCOUNTING_MAX_TENANTS: usize = 4096;

/// Capacity of the accounting bucket and the publication queue.
pub(crate) const SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES: i64 = 64 * 1024 * 1024;

/// Most queued publication messages.
pub(crate) const SNAPSHOT_PUBLICATION_MAX_MESSAGES: i64 = 100_000;

/// Largest queued publication message.
pub(crate) const SNAPSHOT_PUBLICATION_MAX_MESSAGE_BYTES: i32 = 64 * 1024;

/// Room a deletion claim leaves free in the accounting row for later writes.
pub(crate) const SNAPSHOT_ACCOUNTING_ROW_HEADROOM_BYTES: usize = 1024;

/// Times an operator freeze resolution retries after the lease record changes under it.
pub(crate) const FREEZE_RESOLVE_ATTEMPTS: usize = 4;

/// Revisions the drive registry keeps per key.
pub(crate) const DRIVE_KV_HISTORY: i64 = 1;
