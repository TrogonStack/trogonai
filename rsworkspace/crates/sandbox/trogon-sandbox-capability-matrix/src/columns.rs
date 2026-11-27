use std::path::PathBuf;
use std::sync::Arc;

use trogon_sandbox_contract::{ProviderCapabilityProfile, PullSecretId};
use trogon_sandbox_fly_worker::{FlyConfig, FlyConfigParts, FlyPreviewNetwork, FlyRegion};
use trogon_sandbox_modal_worker::{
    DEFAULT_MAX_VCPUS, ModalAppName, ModalConfig, ModalProvider, PythonBridgeModalClient, VolumeSettings,
};
use trogon_sandbox_proxmox_worker::{ProxmoxConfig, ProxmoxConfigParts, VmId};
use trogon_sandbox_space_worker::MaxLeases;
use trogon_sandbox_volumes::VolumeRegion;

use crate::constants::{
    FLY_BOOT_TIMEOUT, FLY_CPUS, FLY_IMAGE, FLY_MAX_LEASES, FLY_MEMORY_MIB, FLY_REGION, FLY_STATE_DIR,
    FLY_WARM_POOL_SIZE, HYPERLIGHT_MAX_CONCURRENT_LEASES, HYPERLIGHT_MAX_WARM_SANDBOXES, MODAL_APP_NAME,
    MODAL_MAX_CONCURRENT_LEASES, MODAL_PYTHON, MODAL_VOLUME_CREDENTIAL_ID, MODAL_VOLUME_REGION, PROXMOX_BOOT_TIMEOUT,
    PROXMOX_IMAGE, PROXMOX_MAX_LEASES, PROXMOX_STATE_DIR, PROXMOX_TEMPLATE, SPACE_MAX_LEASES, SUBPROCESS_PLATFORM_ROWS,
};
use crate::unopened_volume_stores::UnopenedVolumeStores;
use crate::{Column, MatrixError};

/// Every worker's declared profile, one column each.
///
/// Workers that start with operator-chosen settings are rendered at the defaults their binaries document.
/// The networked firecracker variant is its own column because it is a distinct profile. Workers whose
/// profile depends on their config are built from that config alone; no provider, credential, or state
/// directory is created.
pub fn columns() -> Result<Vec<Column>, MatrixError> {
    Ok(vec![
        Column::new("docker", trogon_sandbox_docker_worker::PROFILE.clone()),
        Column::new("k8s", trogon_sandbox_k8s_worker::profile()),
        Column::new("kubevirt", trogon_sandbox_kubevirt_worker::profile()),
        Column::new(
            "hyperlight",
            trogon_sandbox_hyperlight_worker::capability_profile(
                HYPERLIGHT_MAX_WARM_SANDBOXES,
                HYPERLIGHT_MAX_CONCURRENT_LEASES,
            ),
        ),
        Column::new("daytona", trogon_sandbox_daytona_worker::PROFILE.clone()),
        Column::new("aca", trogon_sandbox_aca_worker::PROFILE.clone()),
        Column {
            label: "subprocess",
            profile: trogon_sandbox_subprocess_worker::PROFILE.clone(),
            platform_rows: SUBPROCESS_PLATFORM_ROWS,
        },
        Column::new("substrate", trogon_sandbox_substrate_worker::PROFILE.clone()),
        Column::new("v8-wasm", trogon_sandbox_v8_wasm_worker::PROFILE.clone()),
        Column::new("modal", modal_profile()?),
        Column::new("e2b", trogon_sandbox_e2b_worker::PROFILE.clone()),
        Column::new("vercel", trogon_sandbox_vercel_worker::PROFILE.clone()),
        Column::new("fly", fly_profile()?),
        Column::new("proxmox", proxmox_profile()?),
        Column::new(
            "space",
            trogon_sandbox_space_worker::capability_profile(MaxLeases::parse(SPACE_MAX_LEASES)?),
        ),
        Column::new(
            "firecracker",
            trogon_sandbox_firecracker_worker::profile::PROFILE.clone(),
        ),
        Column::new(
            "firecracker (networked)",
            trogon_sandbox_firecracker_worker::profile::NETWORKED_PROFILE.clone(),
        ),
        Column::new("memory", trogon_sandbox_memory_worker::PROFILE.clone()),
    ])
}

/// The modal profile is derived from its config alone. The bridge and the volume stores exist only because
/// the provider is the public way to reach that profile; neither is called.
fn modal_profile() -> Result<ProviderCapabilityProfile, MatrixError> {
    let config = ModalConfig {
        app_name: ModalAppName::new(MODAL_APP_NAME)?,
        environment: None,
        default_cpu_cores: None,
        max_vcpus: DEFAULT_MAX_VCPUS,
        regions: Vec::new(),
        max_concurrent_leases: MODAL_MAX_CONCURRENT_LEASES,
        volume: VolumeSettings {
            region: VolumeRegion::new(MODAL_VOLUME_REGION)?,
            endpoint: None,
            credential_id: PullSecretId::new(MODAL_VOLUME_CREDENTIAL_ID),
        },
    };
    let api = Arc::new(PythonBridgeModalClient::new(MODAL_PYTHON)?);
    let stores = Arc::new(UnopenedVolumeStores);
    Ok(ModalProvider::new(api, stores, config).profile().clone())
}

/// The fly profile at the binary defaults, in [`FLY_REGION`] because `--region` has none.
fn fly_profile() -> Result<ProviderCapabilityProfile, MatrixError> {
    let config = FlyConfig::new(FlyConfigParts {
        image: FLY_IMAGE.to_owned(),
        region: FlyRegion::new(FLY_REGION)?,
        state_dir: PathBuf::from(FLY_STATE_DIR),
        boot_timeout: FLY_BOOT_TIMEOUT,
        max_leases: FLY_MAX_LEASES,
        memory_mib: FLY_MEMORY_MIB,
        cpus: FLY_CPUS,
        warm_pool_size: FLY_WARM_POOL_SIZE,
        volume: None,
        workspace_volume: None,
        egress_proxy: None,
        public_host: None,
        preview_network: FlyPreviewNetwork::default(),
    })?;
    Ok(trogon_sandbox_fly_worker::capability_profile(&config))
}

/// The proxmox profile at the binary defaults, which leave the guest proxy network off.
fn proxmox_profile() -> Result<ProviderCapabilityProfile, MatrixError> {
    let config = ProxmoxConfig::new(ProxmoxConfigParts {
        template: VmId::try_from(PROXMOX_TEMPLATE)?,
        image: PROXMOX_IMAGE.to_owned(),
        state_dir: PathBuf::from(PROXMOX_STATE_DIR),
        boot_timeout: PROXMOX_BOOT_TIMEOUT,
        max_leases: PROXMOX_MAX_LEASES,
        network: None,
    })?;
    Ok(trogon_sandbox_proxmox_worker::capability_profile(&config))
}

#[cfg(test)]
mod tests;
