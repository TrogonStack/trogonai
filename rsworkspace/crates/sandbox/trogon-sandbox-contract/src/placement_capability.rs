use serde::{Deserialize, Serialize};

use crate::{CapabilitySupport, EnvDigest};

/// Where a worker can place a sandbox and whether it accepts vendor options.
///
/// The default offers no region choice and no passthrough, which is what a local worker declares.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlacementCapability {
    /// Regions in the vendor's own naming. Empty means the worker offers no region choice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
    /// The worker honours validated vendor-native lease options.
    #[serde(default)]
    pub supports_native_passthrough: CapabilitySupport,
    /// The execution environment this worker runs. A restore naming one belongs on a worker
    /// advertising the same digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_digest: Option<EnvDigest>,
}
