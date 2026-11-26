use super::*;

fn failure(r: Result<(), WindowsSandboxError>) -> Result<WindowsSandboxError, Box<dyn std::error::Error>> {
    match r {
        Err(e) => Ok(e),
        Ok(()) => Err("expected an error".into()),
    }
}

#[test]
fn exit_codes_map_to_typed_errors() -> Result<(), Box<dyn std::error::Error>> {
    assert!(check_install_exit(Some(0), "", "").is_ok());
    assert!(check_install_exit(Some(10), "", "").is_ok());
    let cases = [
        (
            12,
            Code::InstallWfpFailed,
            "fence-helper install: WFP filter install failed: boom",
        ),
        (
            14,
            Code::InstallUserFailed,
            "fence-helper install: sandbox user provisioning failed: boom",
        ),
        (13, Code::InstallConfigConflict, "Pass --force to replace"),
        (17, Code::InstallAmbientFailed, "ambient write-deny stamping failed"),
        (1, Code::InstallFailed, "fence-helper install failed (exit 1): boom"),
        (99, Code::InstallFailed, "fence-helper install failed (exit 99): boom"),
    ];
    for (status, code, text) in cases {
        let e = failure(check_install_exit(Some(status), "ignored", "boom"))?;
        assert_eq!(e.code(), code, "{status}");
        assert!(e.message().contains(text), "{status}: {}", e.message());
    }
    Ok(())
}

#[test]
fn a_missing_exit_status_is_a_plain_failure() -> Result<(), Box<dyn std::error::Error>> {
    let e = failure(check_install_exit(None, "", "killed"))?;
    assert_eq!(e.code(), Code::InstallFailed);
    assert!(e.message().contains("(exit none): killed"));
    Ok(())
}

#[test]
fn stdout_is_the_detail_when_stderr_is_empty() -> Result<(), Box<dyn std::error::Error>> {
    let e = failure(check_install_exit(Some(12), "from stdout", ""))?;
    assert!(e.message().ends_with("from stdout"));
    Ok(())
}

#[test]
fn old_fence_helper_is_reported_before_exit_mapping() -> Result<(), Box<dyn std::error::Error>> {
    let e = failure(check_install_exit(
        Some(2),
        "",
        "error: unexpected argument '--allow-loopback' found",
    ))?;
    assert_eq!(e.code(), Code::FenceHelperTooOld);
    Ok(())
}

#[test]
fn timeout_becomes_install_timeout() {
    let e = remap_install_timeout(WindowsSandboxError::new(Code::FenceHelperTimeout, "t"));
    assert_eq!(e.code(), Code::InstallTimeout);
    assert!(
        e.message()
            .starts_with("fence-helper install timed out - the UAC prompt may still be open.")
    );
    let other = remap_install_timeout(WindowsSandboxError::new(Code::SpawnFailed, "s"));
    assert_eq!(other.code(), Code::SpawnFailed);
}

#[test]
fn grant_failures_keep_installed_prefix() {
    let busy = profile_grant_failure(WindowsSandboxError::new(Code::ProfileOperationInProgress, "busy"));
    assert_eq!(busy.code(), Code::ProfileOperationInProgress);
    assert_eq!(busy.message(), "The Windows sandbox is installed. busy");
    let timed = profile_grant_failure(WindowsSandboxError::new(Code::FenceHelperTimeout, "slow"));
    assert_eq!(timed.code(), Code::ProfileGrantFailed);
    assert!(
        timed
            .message()
            .contains("profile did not finish in time: slow\nNo session starts")
    );
    let failed = profile_grant_failure(WindowsSandboxError::new(Code::FenceHelperNonzero, "bad"));
    assert!(failed.message().contains("profile could not be applied: bad"));
}
