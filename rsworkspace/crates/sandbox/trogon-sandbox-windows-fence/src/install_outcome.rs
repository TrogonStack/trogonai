use crate::install_profile_grant::InstallProfileGrant;
use crate::sandbox_status::SandboxStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum InstallOutcome {
    /// The user dismissed the UAC prompt: not an error, the install simply
    /// did not happen.
    Cancelled,
    Installed {
        status: SandboxStatus,
        profile_grant: InstallProfileGrant,
    },
}
