/// The key-value bucket that holds one custody record per source sandbox.
pub(super) const BUCKET: &str = "sandbox-daytona-snapshot-custody";

/// The largest custody record the worker reads or writes.
pub(super) const MAX_RECORD_BYTES: usize = 65_536;

/// The most captures one source sandbox may hold at a time.
pub(super) const MAX_CAPTURES: usize = 128;

/// How often a record update is retried after losing a race with another writer.
pub(super) const MAX_ATTEMPTS: usize = 8;

/// The longest snapshot name the worker accepts.
pub(super) const MAX_NAME_LEN: usize = 256;
