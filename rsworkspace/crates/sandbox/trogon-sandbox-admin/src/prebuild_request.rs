use std::path::PathBuf;
use std::time::Duration;

use trogon_sandbox_contract::TenantId;
use trogon_sandbox_substrate_worker::SandboxClass;

/// What `substrate prebuild` warms and where.
pub struct PrebuildRequest {
    pub ate_api_endpoint: String,
    pub tls_ca_file: Option<PathBuf>,
    pub tls_server_name: Option<String>,
    pub bearer_token_file: Option<PathBuf>,
    pub tenant: TenantId,
    pub image: String,
    pub sandbox_class: SandboxClass,
    pub agent_image: String,
    pub agent_port: u16,
    /// The `ate.dev/size` label the golden build, and therefore the template's own
    /// `worker_selector`, must be pinned to. A worker carries only one such label, and the
    /// template's selector is AND-combined with every actor's, so one image needs a separate
    /// golden snapshot per size class rather than one unconstrained build that could land on
    /// any pool.
    pub size_class_label: String,
    pub snapshots_location: String,
    pub actor_cpu_millicores: u32,
    pub memory_mib: u32,
    pub timeout: Duration,
}
