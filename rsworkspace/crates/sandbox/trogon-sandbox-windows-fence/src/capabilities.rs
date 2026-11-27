use crate::constants::CAPABILITIES_WHY;
use crate::constants::REQUIRED_CAPABILITIES;
use crate::sandbox_user_status::SandboxUserStatus;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// The first capability this library needs that `capabilities` does not list.
pub fn missing_capability(capabilities: &[String]) -> Option<&'static str> {
    REQUIRED_CAPABILITIES
        .into_iter()
        .find(|needed| !capabilities.iter().any(|have| have == needed))
}

/// The typed error for an `fence-helper` that lacks a behaviour by name.
pub fn fence_helper_outdated_error(capability: &str, why: &str) -> WindowsSandboxError {
    WindowsSandboxError::new(
        Code::FenceHelperOutdated,
        format!(
            "The fence-helper in use is older than this library: it does not report the capability \
             '{capability}', which {why}. Update fence-helper (the one shipped with the program \
             embedding this library, or the package's own) to a build that matches the library. \
             Nothing was granted."
        ),
    )
}

/// `FenceHelperOutdated` for the first capability the `fence-helper` that reported
/// `user` lacks.
pub fn check_capabilities(user: &SandboxUserStatus) -> Result<(), WindowsSandboxError> {
    match missing_capability(&user.capabilities) {
        Some(missing) => Err(fence_helper_outdated_error(missing, CAPABILITIES_WHY)),
        None => Ok(()),
    }
}
