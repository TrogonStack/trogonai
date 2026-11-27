pub(super) const BUCKET: &str = "sandbox-e2b-assignments";

pub(super) const MAX_RECORD_BYTES: usize = 8192;

/// Compare-and-swap rounds before a contended record is given up on.
pub(super) const MAX_ROUNDS: usize = 16;

/// The only record version this worker reads or writes.
pub(super) const RECORD_VERSION: u32 = 1;

/// Longest region name kept in recovery metadata, in bytes.
pub(super) const MAX_REGION_BYTES: usize = 128;
