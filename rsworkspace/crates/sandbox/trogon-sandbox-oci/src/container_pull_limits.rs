use crate::constants::DEFAULT_BYTE_BUDGET;

/// Byte budgets for resolving and exporting a container image.
#[derive(Debug, Clone, Copy)]
pub struct ContainerPullLimits {
    pub max_blob_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ContainerPullLimits {
    fn default() -> Self {
        Self {
            max_blob_bytes: DEFAULT_BYTE_BUDGET,
            max_total_bytes: DEFAULT_BYTE_BUDGET,
        }
    }
}
