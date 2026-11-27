use trogon_sandbox_contract::ProviderKind;

pub fn requires_targeted_dispatch(provider: ProviderKind) -> bool {
    matches!(
        provider,
        ProviderKind::Firecracker | ProviderKind::Fly | ProviderKind::Proxmox
    )
}
