use serde::{Deserialize, Serialize};

use crate::{ProviderKind, ProviderVolumeId};

/// A storage volume that lives inside a provider.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderVolumeRef {
    pub provider: ProviderKind,
    pub volume_id: ProviderVolumeId,
}
