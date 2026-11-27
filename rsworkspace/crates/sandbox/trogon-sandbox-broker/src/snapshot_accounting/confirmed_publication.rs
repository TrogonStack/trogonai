use trogon_sandbox_contract::{LiveSnapshotEntry, SnapshotAccountingResponse};

#[derive(Debug, Clone)]
pub struct ConfirmedPublication {
    pub response: SnapshotAccountingResponse,
    pub live: Option<LiveSnapshotEntry>,
    pub projection_revision: u64,
}
