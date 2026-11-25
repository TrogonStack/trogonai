use serde::{Deserialize, Serialize};

use crate::{DriveMount, ProviderVolumeRef};

/// A drive mount resolved to the provider volume that backs it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeVolumeBinding {
    pub mount: DriveMount,
    pub storage: ProviderVolumeRef,
}
