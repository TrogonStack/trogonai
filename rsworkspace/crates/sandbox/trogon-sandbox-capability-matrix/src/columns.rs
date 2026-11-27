use std::sync::Arc;

use trogon_sandbox_contract::PullSecretId;
use trogon_sandbox_modal_worker::{
    DEFAULT_MAX_VCPUS, ModalAppName, ModalConfig, ModalProvider, PythonBridgeModalClient, VolumeSettings,
};
use trogon_sandbox_volumes::VolumeRegion;

use crate::constants::{
    HYPERLIGHT_MAX_CONCURRENT_LEASES, HYPERLIGHT_MAX_WARM_SANDBOXES, MODAL_APP_NAME, MODAL_MAX_CONCURRENT_LEASES,
    MODAL_PYTHON, MODAL_VOLUME_CREDENTIAL_ID, MODAL_VOLUME_REGION, SUBPROCESS_PLATFORM_ROWS,
};
use crate::unopened_volume_stores::UnopenedVolumeStores;
use crate::{Column, MatrixError};

/// Every worker's declared profile, one column each.
///
/// Workers that start with operator-chosen settings are rendered at the defaults their binaries document.
/// The networked firecracker variant is its own column because it is a distinct profile. Workers whose
/// profile needs provider credentials and an on-disk state directory to exist (fly, proxmox, space) are left
/// out.
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
fn modal_profile() -> Result<trogon_sandbox_contract::ProviderCapabilityProfile, MatrixError> {
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

#[cfg(test)]
mod tests;
