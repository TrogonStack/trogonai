use std::fmt;

/// Stable code every [`WindowsSandboxError`](crate::WindowsSandboxError) carries; callers branch on it
/// instead of matching message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowsSandboxErrorCode {
    FenceHelperNotFound,
    SpawnFailed,
    FenceHelperTimeout,
    FenceHelperNonzero,
    FenceHelperBadJson,
    BinShellInvalid,
    WfpVerifyBindFailed,
    WfpVerifyUnparseable,
    WfpFenceInactive,
    WfpVerifyInconclusive,
    WfpLoopbackMismatch,
    FenceHelperTooOld,
    InstallWfpFailed,
    InstallUserFailed,
    InstallConfigConflict,
    InstallAmbientFailed,
    InstallTimeout,
    InstallFailed,
    UninstallFailed,
    ProfileGrantFailed,
    ProfileForgetFailed,
    KeptGrantsInUse,
    ProfileGrantBlockedBySession,
    ProfileOperationInProgress,
    SessionEnded,
    ArgvTooLong,
    NotProvisioned,
    ReinstallRequired,
    MappedDriveCwd,
    SeclogonDisabled,
    TrustCaFailed,
    AclGrantFailed,
    AclStampFailed,
    ProfileGrantOutdated,
    ProfileExclusionReadGrant,
    FenceHelperOutdated,
    TrustCaNotInstalled,
    TrustCaThumbprintMismatch,
}

impl WindowsSandboxErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FenceHelperNotFound => "fence_helper_not_found",
            Self::SpawnFailed => "spawn_failed",
            Self::FenceHelperTimeout => "fence_helper_timeout",
            Self::FenceHelperNonzero => "fence_helper_nonzero",
            Self::FenceHelperBadJson => "fence_helper_bad_json",
            Self::BinShellInvalid => "bin_shell_invalid",
            Self::WfpVerifyBindFailed => "wfp_verify_bind_failed",
            Self::WfpVerifyUnparseable => "wfp_verify_unparseable",
            Self::WfpFenceInactive => "wfp_fence_inactive",
            Self::WfpVerifyInconclusive => "wfp_verify_inconclusive",
            Self::WfpLoopbackMismatch => "wfp_loopback_mismatch",
            Self::FenceHelperTooOld => "fence_helper_too_old",
            Self::InstallWfpFailed => "install_wfp_failed",
            Self::InstallUserFailed => "install_user_failed",
            Self::InstallConfigConflict => "install_config_conflict",
            Self::InstallAmbientFailed => "install_ambient_failed",
            Self::InstallTimeout => "install_timeout",
            Self::InstallFailed => "install_failed",
            Self::UninstallFailed => "uninstall_failed",
            Self::ProfileGrantFailed => "profile_grant_failed",
            Self::ProfileForgetFailed => "profile_forget_failed",
            Self::KeptGrantsInUse => "kept_grants_in_use",
            Self::ProfileGrantBlockedBySession => "profile_grant_blocked_by_session",
            Self::ProfileOperationInProgress => "profile_operation_in_progress",
            Self::SessionEnded => "session_ended",
            Self::ArgvTooLong => "argv_too_long",
            Self::NotProvisioned => "not_provisioned",
            Self::ReinstallRequired => "reinstall_required",
            Self::MappedDriveCwd => "mapped_drive_cwd",
            Self::SeclogonDisabled => "seclogon_disabled",
            Self::TrustCaFailed => "trust_ca_failed",
            Self::AclGrantFailed => "acl_grant_failed",
            Self::AclStampFailed => "acl_stamp_failed",
            Self::ProfileGrantOutdated => "profile_grant_outdated",
            Self::ProfileExclusionReadGrant => "profile_exclusion_read_grant",
            Self::FenceHelperOutdated => "fence_helper_outdated",
            Self::TrustCaNotInstalled => "trust_ca_not_installed",
            Self::TrustCaThumbprintMismatch => "trust_ca_thumbprint_mismatch",
        }
    }
}

impl fmt::Display for WindowsSandboxErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
