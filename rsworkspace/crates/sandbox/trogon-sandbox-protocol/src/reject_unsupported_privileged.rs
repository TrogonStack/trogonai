use trogon_sandbox_contract::{ProviderCapabilityProfile, SandboxSpec};

use crate::privileged_unsupported_error::PrivilegedUnsupportedError;

/// Rejects `spec.privileged()` when the worker's profile says it cannot grant it.
///
/// The wire contract is that a provider without elevated capabilities MUST
/// refuse the request instead of silently running it unprivileged. Every
/// worker's `lease` calls this with its own profile before provisioning.
pub fn reject_unsupported_privileged(
    spec: &SandboxSpec,
    profile: &ProviderCapabilityProfile,
) -> Result<(), PrivilegedUnsupportedError> {
    if spec.privileged().is_granted() && !profile.isolation.supports_privileged.is_supported() {
        return Err(PrivilegedUnsupportedError::new(profile.provider));
    }
    Ok(())
}
