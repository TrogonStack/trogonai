use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    LeaseError, LeaseId, OperationAllocation, OperationCancellation, OperationClaim, OperationCompensationState,
    OperationId, OperationKind, OperationRequest, OperationResult, OperationState, TenantId,
};

/// The broker's durable record of one operation.
///
/// Optional fields are omitted from JSON when absent and default when an older record leaves them out. Times use
/// serde's `SystemTime` encoding, an object of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: OperationId,
    pub tenant: TenantId,
    pub kind: OperationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LeaseId>,
    /// A hash of the request, so reusing the id for a different request is caught.
    pub request_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<OperationRequest>,
    pub state: OperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation: Option<OperationCancellation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allocation: Option<OperationAllocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<OperationCompensationState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<OperationClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<OperationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LeaseError>,
    pub requested_at: SystemTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_after: Option<SystemTime>,
}
