pub(super) const BUCKET: &str = "sandbox-e2b-snapshot-custody";

pub(super) const MAX_RECORD_BYTES: usize = 4096;

/// Stream sequences examined per scan, so one scan stays bounded.
pub(super) const SCAN_BATCH: u64 = 128;

/// Cleanup attempts the background sweep makes before leaving a snapshot for a manual delete.
pub(crate) const MAX_AUTOMATIC_ATTEMPTS: u32 = 8;
