use std::time::Duration;

/// Default KV bucket for operation records.
pub const OPERATIONS_BUCKET: &str = "sandbox-operations";

/// How long a finished operation stays readable before a sweeper may drop it.
pub const OPERATION_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// A claim from another owner older than this is taken over by the dispatcher
/// tick. Startup recovery takes over every foreign claim at once.
pub const STALE_CLAIM_AFTER: Duration = Duration::from_secs(60);

/// Safe transient failures give up after this many attempts; unresolved
/// resource effects retain their operation until they settle.
pub const MAX_ATTEMPTS: u32 = 16;

/// Wait before a re-driven operation that failed transiently is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(5);

pub(super) const CAS_RETRIES: usize = 8;
