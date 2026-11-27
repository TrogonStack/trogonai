use super::*;

pub struct BrokerConfig {
    pub default_cpu_cap: u32,
    pub default_wall_cap: u32,
    pub enforce_image_allowlist: bool,
    pub enforce_worker_registry: bool,
    pub default_max_cpu_per_request: u32,
    pub default_max_memory_mib_per_request: u32,
    pub default_active_lease_cap: u32,
    pub snapshot_registry: String,
    /// Deadline for the provider dispatch round trip. Providers such as the substrate
    /// worker can take far longer than a typical request-reply exchange on a cold
    /// golden-snapshot build, so this needs its own generous budget.
    pub provider_dispatch_timeout: Duration,
}
