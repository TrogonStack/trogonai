use serde::{Deserialize, Serialize};

use crate::DormancyCapability;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct LifecycleCapability {
    pub supports_warm_pool: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_warm_pool_size: Option<u32>,
    pub supports_named_tags: bool,
    pub supports_prewarm_per_tenant: bool,
    /// Freezes a running lease in place and thaws it later with the same lease id and state.
    #[serde(default)]
    pub supports_pause_resume: bool,
    /// The vendor stops an idle sandbox on its own, honouring the lease's idle timeout.
    #[serde(default)]
    pub supports_native_auto_stop: bool,
    /// Longest single lease the vendor runs, in seconds. `None` means the worker imposes no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lease_lifetime_seconds: Option<u32>,
    /// Which form of pause the worker performs; `supports_pause_resume` only says some form exists.
    #[serde(default)]
    pub dormancy: DormancyCapability,
}
