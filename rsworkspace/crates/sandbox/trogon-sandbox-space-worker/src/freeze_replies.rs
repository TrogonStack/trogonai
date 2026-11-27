use trogon_sandbox_contract::{SandboxFreezeResponse, SandboxSuspendResponse, SandboxWakeResponse};

/// The suspend reply for the outcome of a freeze; no artifact is produced because the
/// sandbox stays on SPACE.
pub(crate) fn suspend_reply(freeze: SandboxFreezeResponse) -> SandboxSuspendResponse {
    match freeze {
        SandboxFreezeResponse::Done => SandboxSuspendResponse::Done {
            artifact: None,
            artifact_bytes: None,
        },
        SandboxFreezeResponse::Unsupported => SandboxSuspendResponse::Unsupported,
        SandboxFreezeResponse::Denied(error) => SandboxSuspendResponse::Denied(error),
    }
}

/// The wake reply for the outcome of a freeze.
pub(crate) fn wake_reply(freeze: SandboxFreezeResponse) -> SandboxWakeResponse {
    match freeze {
        SandboxFreezeResponse::Done => SandboxWakeResponse::Done,
        SandboxFreezeResponse::Unsupported => SandboxWakeResponse::Unsupported,
        SandboxFreezeResponse::Denied(error) => SandboxWakeResponse::Denied(error),
    }
}

#[cfg(test)]
mod tests;
