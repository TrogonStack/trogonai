use serde::{Deserialize, Serialize};

use crate::{NativeSnapshotRef, TenantId};

/// Client-facing request to delete a provider-native snapshot.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSnapshotDeleteRequest {
    pub tenant: TenantId,
    pub reference: NativeSnapshotRef,
}
