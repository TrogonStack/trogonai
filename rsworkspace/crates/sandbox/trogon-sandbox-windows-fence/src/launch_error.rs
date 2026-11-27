use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

pub(crate) fn seclogon_message(state: &str) -> &'static str {
    if state == "missing" {
        "The Windows Secondary Logon service (seclogon) is not present on this machine; the \
         sandbox cannot start without it."
    } else {
        "The Windows Secondary Logon service (seclogon) is disabled; an administrator must set \
         its startup type to Manual (`sc.exe config seclogon start= demand`). The sandbox cannot \
         start without it."
    }
}

/// The typed error a runner-launching subcommand reports as one JSON line
/// on stderr, if `stderr` carries one.
pub fn parse_launch_error(stderr: &str, subcommand: &str) -> Option<WindowsSandboxError> {
    for line in stderr.lines() {
        let t = line.trim();
        if !t.starts_with('{') || !t.contains("\"code\"") {
            continue;
        }
        let Ok(j) = serde_json::from_str::<serde_json::Value>(t) else {
            continue;
        };
        let text = |k: &str| j.get(k).and_then(|v| v.as_str());
        let (code, message, sub, drive, state) = match text("code") {
            Some("profile_operation_in_progress") => (
                Code::ProfileOperationInProgress,
                text("message")
                    .unwrap_or("a profile grant or forget is in progress")
                    .to_string(),
                "exec",
                None,
                None,
            ),
            Some("session_ended") => (
                Code::SessionEnded,
                text("message")
                    .unwrap_or("the session that started this command ended; the command was terminated")
                    .to_string(),
                "exec",
                None,
                None,
            ),
            Some("mapped_drive_cwd") => (
                Code::MappedDriveCwd,
                text("message")
                    .unwrap_or("mapped/network-drive working directory")
                    .to_string(),
                subcommand,
                text("drive").map(str::to_string),
                None,
            ),
            Some("wfp_loopback_mismatch") => (
                Code::WfpLoopbackMismatch,
                text("message")
                    .unwrap_or("the installed loopback mode is not this session's")
                    .to_string(),
                "exec",
                None,
                None,
            ),
            Some("seclogon_disabled") => {
                let state = if text("state") == Some("missing") {
                    "missing"
                } else {
                    "disabled"
                };
                (
                    Code::SeclogonDisabled,
                    text("message")
                        .map(str::to_string)
                        .unwrap_or_else(|| seclogon_message(state).to_string()),
                    subcommand,
                    None,
                    Some(state),
                )
            }
            _ => continue,
        };
        let error = WindowsSandboxError::for_subcommand(code, message, sub).with_drive(drive);
        return Some(match state {
            Some(state) => error.with_state(state),
            None => error,
        });
    }
    None
}
