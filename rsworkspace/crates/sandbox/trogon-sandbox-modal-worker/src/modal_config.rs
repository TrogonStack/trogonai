use crate::{CpuCores, ModalAppName, VolumeSettings};

/// Everything the worker needs that comes from the operator rather than from a lease request.
#[derive(Debug, Clone)]
pub struct ModalConfig {
    pub app_name: ModalAppName,
    pub environment: Option<String>,
    /// Modal's `cpu` is a core request, so it is configured per worker rather than derived from CPU seconds.
    pub default_cpu_cores: Option<CpuCores>,
    pub max_vcpus: u32,
    pub regions: Vec<String>,
    pub max_concurrent_leases: u32,
    pub volume: VolumeSettings,
}
