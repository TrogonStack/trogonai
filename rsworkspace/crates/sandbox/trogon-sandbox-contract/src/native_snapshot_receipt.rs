use serde::{Deserialize, Serialize};

use crate::NativeSnapshotRef;

/// What a provider-native snapshot returns in place of an OCI receipt.
///
/// `bytes` is omitted when the provider does not report a size.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSnapshotReceipt {
    pub reference: NativeSnapshotRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}
