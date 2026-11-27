use trogon_sandbox_contract::SnapshotAccountingRefusal;

/// Result of an accounting step; the error is the refusal the contract defines.
pub(super) type Result<T> = std::result::Result<T, SnapshotAccountingRefusal>;
