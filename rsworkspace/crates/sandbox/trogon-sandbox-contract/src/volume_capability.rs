use serde::{Deserialize, Serialize};

use crate::{CapabilitySupport, VolumeMode};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct VolumeCapability {
    pub supported_modes: Vec<VolumeMode>,
    pub supports_s3_backend: CapabilitySupport,
    #[serde(default)]
    pub supports_fuse_mounts: CapabilitySupport,
}
