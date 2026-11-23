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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationTier {
    Container,
    Bwrap,
    UserSpaceKernel,
    MicroVm,
    FullVm,
    WasmMicroVm,
}

impl IsolationTier {
    pub fn providers_supporting(self) -> &'static [ProviderKind] {
        match self {
            Self::Container => &[
                ProviderKind::Subprocess,
                ProviderKind::LocalDocker,
                ProviderKind::Daytona,
            ],
            Self::Bwrap => &[ProviderKind::Subprocess],
            Self::UserSpaceKernel => &[ProviderKind::K8sMicroVm, ProviderKind::Substrate, ProviderKind::Modal],
            Self::MicroVm => &[
                ProviderKind::K8sMicroVm,
                ProviderKind::AzureContainerApps,
                ProviderKind::Substrate,
                ProviderKind::Firecracker,
                ProviderKind::Fly,
                ProviderKind::Proxmox,
                ProviderKind::E2b,
                ProviderKind::Vercel,
                ProviderKind::Space,
            ],
            Self::FullVm => &[ProviderKind::KubeVirt, ProviderKind::Proxmox],
            Self::WasmMicroVm => &[ProviderKind::V8Wasm, ProviderKind::Hyperlight],
        }
    }
}

impl ProviderKind {
    pub fn isolation_tiers(self) -> &'static [IsolationTier] {
        match self {
            Self::Subprocess => &[IsolationTier::Container, IsolationTier::Bwrap],
            Self::LocalDocker | Self::Daytona => &[IsolationTier::Container],
            Self::Modal => &[IsolationTier::UserSpaceKernel],
            Self::K8sMicroVm => &[IsolationTier::UserSpaceKernel, IsolationTier::MicroVm],
            Self::AzureContainerApps => &[IsolationTier::MicroVm],
            Self::Substrate => &[IsolationTier::UserSpaceKernel, IsolationTier::MicroVm],
            Self::Firecracker => &[IsolationTier::MicroVm],
            Self::Fly | Self::E2b | Self::Vercel | Self::Space => &[IsolationTier::MicroVm],
            Self::Proxmox => &[IsolationTier::MicroVm, IsolationTier::FullVm],
            Self::KubeVirt => &[IsolationTier::FullVm],
            Self::Hyperlight | Self::V8Wasm => &[IsolationTier::WasmMicroVm],
            Self::Mock => &[
                IsolationTier::Container,
                IsolationTier::Bwrap,
                IsolationTier::UserSpaceKernel,
                IsolationTier::MicroVm,
                IsolationTier::FullVm,
                IsolationTier::WasmMicroVm,
            ],
        }
    }

    pub fn supports_tier(self, tier: IsolationTier) -> bool {
        self.isolation_tiers().contains(&tier)
    }
}
