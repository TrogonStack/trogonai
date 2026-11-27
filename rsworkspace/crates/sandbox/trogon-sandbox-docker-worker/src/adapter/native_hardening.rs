use super::*;

/// The worker's hardening with the lease's `native.local_docker` options laid
/// over it. `Ok(None)` means the lease carries none, so the worker's own
/// hardening applies untouched. Anything outside [`NATIVE_OPTION_KEYS`], or
/// out of range, is refused.
pub(crate) fn hardening_with_native(
    base: &ContainerHardening,
    native: Option<&serde_json::Value>,
) -> Result<Option<ContainerHardening>, trogon_sandbox_protocol::NativeOptionsError> {
    let options = trogon_sandbox_protocol::options_for(ProviderKind::LocalDocker, native)?;
    if options.is_empty() {
        return Ok(None);
    }
    trogon_sandbox_protocol::reject_unknown_keys(&options, NATIVE_OPTION_KEYS)?;
    let mut hardening = base.clone();
    if let Some(pids) = trogon_sandbox_protocol::optional_u64(&options, "pids_limit", 1, 1 << 20)? {
        hardening.pids_limit = Some(pids as i64);
    }
    if let Some(shm) = trogon_sandbox_protocol::optional_u64(&options, "shm_size_mib", 1, 65_536)? {
        hardening.shm_size_mib = Some(shm);
    }
    Ok(Some(hardening))
}
