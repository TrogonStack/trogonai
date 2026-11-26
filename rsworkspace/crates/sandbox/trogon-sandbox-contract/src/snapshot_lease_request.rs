use serde::{Deserialize, Serialize};

use crate::{LeaseId, NativeSnapshotAcceptance, TenantId};

/// Client-facing request to capture a snapshot of a running lease without stopping it.
///
/// The receipt can seed a new lease as its image, so it is the uniform lease-start source across providers that can
/// restore it. `accept_native` says whether the caller can read a
/// [`SnapshotLeaseResponse::CreatedNative`](crate::SnapshotLeaseResponse::CreatedNative) reply. Callers built before
/// native snapshots omit it, which reads as `Declined`, and get `Unsupported` from a provider that can only snapshot
/// natively.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotLeaseRequest {
    pub tenant: TenantId,
    pub lease_id: LeaseId,
    #[serde(default)]
    pub accept_native: NativeSnapshotAcceptance,
}
