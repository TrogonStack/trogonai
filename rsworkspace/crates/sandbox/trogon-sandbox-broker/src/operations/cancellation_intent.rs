use trogon_sandbox_contract::{OperationCancellation, OperationId, TenantId};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CancellationIntent {
    pub tenant: TenantId,
    pub operation_id: OperationId,
    pub cancellation: OperationCancellation,
}
