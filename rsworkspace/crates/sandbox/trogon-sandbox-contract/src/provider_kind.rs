use serde::{Deserialize, Serialize};

/// Provider identity carried in sandbox wire messages.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    K8sMicroVm,
    Subprocess,
    LocalDocker,
    Daytona,
    Modal,
    Hyperlight,
    V8Wasm,
    KubeVirt,
    AzureContainerApps,
    Substrate,
    Firecracker,
    Proxmox,
    Fly,
    E2b,
    Vercel,
    Space,
    Mock,
}
