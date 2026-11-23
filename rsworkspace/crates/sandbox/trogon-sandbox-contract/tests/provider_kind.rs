use std::collections::HashSet;

use serde_json::error::Category::{Data, Syntax};
use trogon_sandbox_contract::ProviderKind;

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
