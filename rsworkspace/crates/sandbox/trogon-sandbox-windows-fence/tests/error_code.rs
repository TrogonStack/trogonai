use trogon_sandbox_windows_fence::WindowsSandboxErrorCode as Code;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_code_has_a_stable_name_that_displays_the_same() -> TestResult {
    let all = [
        (Code::FenceHelperNotFound, "fence_helper_not_found"),
        (Code::SpawnFailed, "spawn_failed"),
        (Code::FenceHelperTimeout, "fence_helper_timeout"),
        (Code::FenceHelperNonzero, "fence_helper_nonzero"),
        (Code::FenceHelperBadJson, "fence_helper_bad_json"),
        (Code::BinShellInvalid, "bin_shell_invalid"),
        (Code::WfpVerifyBindFailed, "wfp_verify_bind_failed"),
        (Code::WfpVerifyUnparseable, "wfp_verify_unparseable"),
        (Code::WfpFenceInactive, "wfp_fence_inactive"),
        (Code::WfpVerifyInconclusive, "wfp_verify_inconclusive"),
        (Code::WfpLoopbackMismatch, "wfp_loopback_mismatch"),
        (Code::FenceHelperTooOld, "fence_helper_too_old"),
        (Code::InstallWfpFailed, "install_wfp_failed"),
        (Code::InstallUserFailed, "install_user_failed"),
        (Code::InstallConfigConflict, "install_config_conflict"),
        (Code::InstallAmbientFailed, "install_ambient_failed"),
        (Code::InstallTimeout, "install_timeout"),
        (Code::InstallFailed, "install_failed"),
        (Code::UninstallFailed, "uninstall_failed"),
        (Code::ProfileGrantFailed, "profile_grant_failed"),
        (Code::ProfileForgetFailed, "profile_forget_failed"),
        (Code::KeptGrantsInUse, "kept_grants_in_use"),
        (Code::ProfileGrantBlockedBySession, "profile_grant_blocked_by_session"),
        (Code::ProfileOperationInProgress, "profile_operation_in_progress"),
        (Code::SessionEnded, "session_ended"),
        (Code::ArgvTooLong, "argv_too_long"),
        (Code::NotProvisioned, "not_provisioned"),
        (Code::ReinstallRequired, "reinstall_required"),
        (Code::MappedDriveCwd, "mapped_drive_cwd"),
        (Code::SeclogonDisabled, "seclogon_disabled"),
        (Code::TrustCaFailed, "trust_ca_failed"),
        (Code::AclGrantFailed, "acl_grant_failed"),
        (Code::AclStampFailed, "acl_stamp_failed"),
        (Code::ProfileGrantOutdated, "profile_grant_outdated"),
        (Code::ProfileExclusionReadGrant, "profile_exclusion_read_grant"),
        (Code::FenceHelperOutdated, "fence_helper_outdated"),
        (Code::TrustCaNotInstalled, "trust_ca_not_installed"),
        (Code::TrustCaThumbprintMismatch, "trust_ca_thumbprint_mismatch"),
    ];
    for (code, name) in all {
        assert_eq!(code.as_str(), name);
        assert_eq!(code.to_string(), name);
    }
    Ok(())
}
