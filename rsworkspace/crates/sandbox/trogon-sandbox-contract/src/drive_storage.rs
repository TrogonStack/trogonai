use serde::{Deserialize, Serialize};

use crate::ProviderVolumeRef;

/// Where a drive's data lives.
///
/// It is untagged: `S3` is the object `{"bucket": ...}` and `ProviderVolume` is
/// `{"provider_volume": {...}}`. Unknown fields are rejected.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum DriveStorage {
    S3 { bucket: String },
    ProviderVolume { provider_volume: ProviderVolumeRef },
}

impl DriveStorage {
    pub fn s3_bucket(&self) -> Option<&str> {
        match self {
            Self::S3 { bucket } => Some(bucket),
            Self::ProviderVolume { .. } => None,
        }
    }

    pub fn provider_volume(&self) -> Option<&ProviderVolumeRef> {
        match self {
            Self::ProviderVolume { provider_volume } => Some(provider_volume),
            Self::S3 { .. } => None,
        }
    }
}
