use serde::{Deserialize, Serialize};

use crate::ResourceRange;

/// Machine-size ranges a worker can honour.
///
/// `None` means the worker cannot enforce that resource at all, so a request carrying it must be
/// refused instead of running on an unsized machine.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcpu_millis: Option<ResourceRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_mib: Option<ResourceRange>,
    /// Live CPU and memory readings are available for a running lease.
    #[serde(default)]
    pub reports_usage_gauges: bool,
}
