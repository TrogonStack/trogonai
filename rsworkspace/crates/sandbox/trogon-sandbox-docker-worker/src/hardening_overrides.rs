use trogon_sandbox_contract::ContainerOverrides;
use trogon_sandbox_providers::ContainerHardening;

/// The worker's hardening with a lease's overrides applied.
///
/// This never touches `no_new_privileges`, the memory cap or the swap pin. Capability names are parsed
/// (`ExtraCapability`) before they reach a lease, so the deny list is enforced by the type and a name that would
/// escape the container cannot appear here.
pub fn hardening_with_overrides(base: &ContainerHardening, overrides: &ContainerOverrides) -> ContainerHardening {
    let mut hardening = base.clone();
    if let Some(pids) = overrides.pids_limit {
        hardening.pids_limit = Some(i64::from(pids.get()));
    }
    if overrides.writable_rootfs.is_writable() {
        hardening.readonly_rootfs = false;
    }
    if let Some(shm) = overrides.shm_size_mib {
        hardening.shm_size_mib = Some(u64::from(shm.get()));
    }
    for cap in &overrides.extra_capabilities {
        let name = cap.as_str();
        if !hardening.retained_capabilities.iter().any(|kept| kept == name) {
            hardening.retained_capabilities.push(name.to_owned());
        }
    }
    hardening
}

#[cfg(test)]
mod tests;
