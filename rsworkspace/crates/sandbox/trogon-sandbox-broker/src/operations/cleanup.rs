use trogon_sandbox_contract::{DriveId, OperationAllocation, OperationId, TenantId};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CleanupPhase {
    Requested,
    ProviderConfirmed,
    Settled,
}

#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnedCleanup {
    pub(super) version: u8,
    pub(super) tenant: TenantId,
    pub(super) operation_id: OperationId,
    pub allocation: OperationAllocation,
    pub phase: CleanupPhase,
    pub name: Option<String>,
    pub drives: Vec<DriveId>,
    pub monitoring: bool,
}
