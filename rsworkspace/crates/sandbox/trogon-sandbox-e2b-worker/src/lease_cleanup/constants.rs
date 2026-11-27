pub(super) const BUCKET: &str = "sandbox-e2b-lease-cleanup";

pub(super) const MAX_RECORD_BYTES: usize = 4096;

/// Rounds `begin` spends racing other writers before it gives up.
pub(super) const MAX_BEGIN_ROUNDS: usize = 8;

/// The only receipt version this worker reads or writes.
pub(super) const RECEIPT_VERSION: u32 = 1;

/// Longest lease identity a cleanup may name, in bytes.
pub(super) const MAX_LEASE_ID_BYTES: usize = 1024;
