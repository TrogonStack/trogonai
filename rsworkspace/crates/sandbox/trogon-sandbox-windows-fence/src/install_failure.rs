use crate::exit_code::ExitCode;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

pub(crate) fn fence_helper_too_old(stderr: &str) -> WindowsSandboxError {
    WindowsSandboxError::new(
        Code::FenceHelperTooOld,
        format!(
            "fence-helper is too old for opening loopback: it does not know \
             `--allow-loopback`. Update fence-helper (and re-run the install with it), or keep \
             loopback closed. ({})",
            stderr.trim()
        ),
    )
}

/// Map the exit of `fence-helper install` (0 ok, 10 UAC cancelled, 12 WFP failed,
/// 13 config conflict, 14 user provisioning failed, 17 ambient stamping
/// failed, anything else other). The caller reads state back on 0 and 10.
pub(crate) fn check_install_exit(status: Option<i32>, stdout: &str, stderr: &str) -> Result<(), WindowsSandboxError> {
    if status == Some(0) || ExitCode::UAC_CANCELLED.matches(status) {
        return Ok(());
    }
    if stderr.contains("unexpected argument '--allow-loopback'") {
        return Err(fence_helper_too_old(stderr));
    }
    let out = if stderr.is_empty() { stdout } else { stderr };
    let (code, message) = if ExitCode::WFP_FAILED.matches(status) {
        (
            Code::InstallWfpFailed,
            format!("fence-helper install: WFP filter install failed: {out}"),
        )
    } else if ExitCode::USER_FAILED.matches(status) {
        (
            Code::InstallUserFailed,
            format!("fence-helper install: sandbox user provisioning failed: {out}"),
        )
    } else if ExitCode::CONFIG_CONFLICT.matches(status) {
        (
            Code::InstallConfigConflict,
            format!(
                "fence-helper install: filters already exist under this sublayer with a different \
                 port range, loopback mode or sandbox-user name. Pass --force to replace, or \
                 pick a different sublayer GUID. Output: {out}"
            ),
        )
    } else if ExitCode::AMBIENT_FAILED.matches(status) {
        (
            Code::InstallAmbientFailed,
            format!(
                "fence-helper install: ambient write-deny stamping failed (stock world-writable \
                 system dirs could not be deny-stamped for the sandbox user): {out}"
            ),
        )
    } else {
        (
            Code::InstallFailed,
            format!(
                "fence-helper install failed (exit {}): {out}",
                status.map_or_else(|| "none".to_string(), |s| s.to_string())
            ),
        )
    };
    Err(WindowsSandboxError::new(code, message))
}

/// Re-map a timeout of the install spawn to the more specific
/// `install_timeout` (the UAC prompt is the usual cause).
pub(crate) fn remap_install_timeout(e: WindowsSandboxError) -> WindowsSandboxError {
    if e.code() != Code::FenceHelperTimeout {
        return e;
    }
    WindowsSandboxError::new(
        Code::InstallTimeout,
        format!(
            "fence-helper install timed out - the UAC prompt may still be open. Re-run when ready to \
             grant elevation. ({})",
            e.message()
        ),
    )
}

/// The account and the filters are installed and the profile read grant is
/// not: its own error, so the caller does not take the whole install for
/// failed.
pub(crate) fn profile_grant_failure(cause: WindowsSandboxError) -> WindowsSandboxError {
    if matches!(
        cause.code(),
        Code::ProfileGrantBlockedBySession | Code::ProfileOperationInProgress
    ) {
        return WindowsSandboxError::for_subcommand(
            cause.code(),
            format!("The Windows sandbox is installed. {}", cause.message()),
            "profile",
        );
    }
    let how = if cause.code() == Code::FenceHelperTimeout {
        "did not finish in time"
    } else {
        "could not be applied"
    };
    WindowsSandboxError::for_subcommand(
        Code::ProfileGrantFailed,
        format!(
            "The Windows sandbox is installed, but the read grant on your user profile {how}: \
             {}\nNo session starts until it is complete. Run the install again, or `fence-helper \
             profile grant`, to finish it; `fence-helper profile forget` removes what was written.",
            cause.message()
        ),
        "profile",
    )
}

#[cfg(test)]
mod tests;
