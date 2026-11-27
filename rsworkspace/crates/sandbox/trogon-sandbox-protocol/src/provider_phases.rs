use trogon_sandbox_contract::{ProviderKind, SandboxPhase};

/// The phases a UI may show for `provider`, in the order the provider performs them.
///
/// `RestoreSnapshot` is advertised only by providers that boot from a snapshot.
pub fn phases_supported_by(provider: ProviderKind) -> &'static [SandboxPhase] {
    match provider {
        ProviderKind::LocalDocker => &[
            SandboxPhase::Allocate,
            SandboxPhase::PrepareWorkspace,
            SandboxPhase::MountFilesystem,
            SandboxPhase::PullingImage,
            SandboxPhase::BootRuntime,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::K8sMicroVm
        | ProviderKind::Daytona
        | ProviderKind::Modal
        | ProviderKind::E2b
        | ProviderKind::Vercel
        | ProviderKind::Space
        | ProviderKind::KubeVirt
        | ProviderKind::Proxmox
        | ProviderKind::Fly => &[
            SandboxPhase::Allocate,
            SandboxPhase::PrepareWorkspace,
            SandboxPhase::BootRuntime,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::Subprocess | ProviderKind::AzureContainerApps => {
            &[SandboxPhase::Allocate, SandboxPhase::BootRuntime, SandboxPhase::Reclaim]
        }
        ProviderKind::Hyperlight => &[
            SandboxPhase::Allocate,
            SandboxPhase::RestoreSnapshot,
            SandboxPhase::Execute,
            SandboxPhase::Snapshotting,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::Hluk => &[
            SandboxPhase::Allocate,
            SandboxPhase::PrepareWorkspace,
            SandboxPhase::RestoreSnapshot,
            SandboxPhase::Execute,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::V8Wasm => &[
            SandboxPhase::Allocate,
            SandboxPhase::BootRuntime,
            SandboxPhase::Execute,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::Substrate => &[
            SandboxPhase::Allocate,
            SandboxPhase::PullingImage,
            SandboxPhase::RestoreSnapshot,
            SandboxPhase::BootRuntime,
            SandboxPhase::PrepareWorkspace,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::Firecracker => &[
            SandboxPhase::Allocate,
            SandboxPhase::PullingImage,
            SandboxPhase::RestoreSnapshot,
            SandboxPhase::PrepareWorkspace,
            SandboxPhase::BootRuntime,
            SandboxPhase::Reclaim,
        ],
        ProviderKind::Mock => &[SandboxPhase::Allocate],
    }
}
