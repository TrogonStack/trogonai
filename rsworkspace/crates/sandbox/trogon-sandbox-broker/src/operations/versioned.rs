use trogon_sandbox_contract::OperationRecord;

/// A record together with the revision it was read at.
#[derive(Debug, Clone)]
pub struct Versioned {
    pub record: OperationRecord,
    pub revision: u64,
}
