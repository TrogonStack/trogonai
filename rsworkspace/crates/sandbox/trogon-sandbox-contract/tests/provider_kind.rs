use std::collections::HashSet;

use serde_json::error::Category::{Data, Syntax};
use trogon_sandbox_contract::{IsolationTier, ProviderKind};

#[test]
fn every_provider_kind_preserves_its_json_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = [
        (ProviderKind::K8sMicroVm, "k8s_micro_vm"),
        (ProviderKind::Subprocess, "subprocess"),
        (ProviderKind::LocalDocker, "local_docker"),
        (ProviderKind::Daytona, "daytona"),
        (ProviderKind::Modal, "modal"),
        (ProviderKind::Hyperlight, "hyperlight"),
        (ProviderKind::V8Wasm, "v8_wasm"),
        (ProviderKind::KubeVirt, "kube_virt"),
        (ProviderKind::AzureContainerApps, "azure_container_apps"),
        (ProviderKind::Substrate, "substrate"),
        (ProviderKind::Firecracker, "firecracker"),
        (ProviderKind::Proxmox, "proxmox"),
        (ProviderKind::Fly, "fly"),
        (ProviderKind::E2b, "e2b"),
        (ProviderKind::Vercel, "vercel"),
        (ProviderKind::Space, "space"),
        (ProviderKind::Hluk, "hluk"),
        (ProviderKind::Mock, "mock"),
    ];
    let mut unique = HashSet::new();
    for (provider, spelling) in fixtures {
        assert!(unique.insert(provider));
        let json = serde_json::to_string(&provider)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<ProviderKind>(&json)?, provider);
    }
    Ok(())
}

#[test]
fn unsupported_provider_kinds_and_json_shapes_are_rejected() {
    for (json, category) in [
        ("\"K8sMicroVm\"", Data),
        ("\"k8s_Kicro_vm\"", Data),
        ("\"k8s_microvm\"", Data),
        ("\"kubevirt\"", Data),
        ("\"unknown\"", Data),
        ("null", Syntax),
        ("true", Syntax),
        ("0", Syntax),
        ("[]", Syntax),
        ("{}", Syntax),
    ] {
        let actual = serde_json::from_str::<ProviderKind>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(category), "{json}");
    }
}

#[test]
fn every_isolation_tier_preserves_its_json_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = [
        (IsolationTier::Container, "container"),
        (IsolationTier::Bwrap, "bwrap"),
        (IsolationTier::UserSpaceKernel, "user_space_kernel"),
        (IsolationTier::MicroVm, "micro_vm"),
        (IsolationTier::FullVm, "full_vm"),
        (IsolationTier::WasmMicroVm, "wasm_micro_vm"),
    ];
    for (tier, spelling) in fixtures {
        let json = serde_json::to_string(&tier)?;
        assert_eq!(json, format!("\"{spelling}\""));
        assert_eq!(serde_json::from_str::<IsolationTier>(&json)?, tier);
    }
    Ok(())
}

#[test]
fn provider_and_tier_mappings_are_bidirectional() {
    let expected = [
        (
            IsolationTier::Container,
            &[
                ProviderKind::Subprocess,
                ProviderKind::LocalDocker,
                ProviderKind::Daytona,
            ][..],
        ),
        (IsolationTier::Bwrap, &[ProviderKind::Subprocess][..]),
        (
            IsolationTier::UserSpaceKernel,
            &[ProviderKind::K8sMicroVm, ProviderKind::Substrate, ProviderKind::Modal][..],
        ),
        (
            IsolationTier::MicroVm,
            &[
                ProviderKind::K8sMicroVm,
                ProviderKind::AzureContainerApps,
                ProviderKind::Substrate,
                ProviderKind::Firecracker,
                ProviderKind::Fly,
                ProviderKind::Proxmox,
                ProviderKind::E2b,
                ProviderKind::Vercel,
                ProviderKind::Space,
                ProviderKind::Hluk,
            ][..],
        ),
        (
            IsolationTier::FullVm,
            &[ProviderKind::KubeVirt, ProviderKind::Proxmox][..],
        ),
        (
            IsolationTier::WasmMicroVm,
            &[ProviderKind::V8Wasm, ProviderKind::Hyperlight][..],
        ),
    ];
    for (tier, providers) in expected {
        assert_eq!(tier.providers_supporting(), providers);
        for provider in providers {
            assert!(provider.supports_tier(tier));
            assert!(provider.isolation_tiers().contains(&tier));
        }
    }
    assert!(ProviderKind::Mock.supports_tier(IsolationTier::WasmMicroVm));
    assert!(
        ProviderKind::Mock
            .isolation_tiers()
            .contains(&IsolationTier::WasmMicroVm)
    );
}

#[test]
fn unsupported_isolation_tiers_are_rejected() {
    for json in [
        "\"user-space-kernel\"",
        "\"microvm\"",
        "\"fullvm\"",
        "\"unknown\"",
        "null",
        "true",
        "0",
        "[]",
        "{}",
    ] {
        assert!(serde_json::from_str::<IsolationTier>(json).is_err(), "decoded {json}");
    }
}
