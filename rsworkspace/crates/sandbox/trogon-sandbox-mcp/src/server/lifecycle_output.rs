use trogon_sandbox_contract::{PauseLeaseResponse, ResumeLeaseResponse, SuspendLeaseResponse, WakeLeaseResponse};

use crate::inputs::lifecycle_inputs::{PauseLeaseOutput, ResumeLeaseOutput, SuspendLeaseOutput, WakeLeaseOutput};
use crate::tool_error::ToolError;

pub(super) fn pause_output(response: PauseLeaseResponse) -> PauseLeaseOutput {
    match response {
        PauseLeaseResponse::Paused { reap_at, .. } => PauseLeaseOutput {
            outcome: "paused".into(),
            reap_at: Some(reap_at),
        },
        PauseLeaseResponse::Unsupported | PauseLeaseResponse::Denied(_) => PauseLeaseOutput {
            outcome: "unsupported".into(),
            reap_at: None,
        },
    }
}

pub(super) fn resume_output(response: ResumeLeaseResponse) -> ResumeLeaseOutput {
    match response {
        ResumeLeaseResponse::Resumed { deadline, .. } => ResumeLeaseOutput {
            outcome: "resumed".into(),
            deadline: Some(deadline),
        },
        ResumeLeaseResponse::Unsupported | ResumeLeaseResponse::Denied(_) => ResumeLeaseOutput {
            outcome: "unsupported".into(),
            deadline: None,
        },
    }
}

pub(super) fn suspend_output(response: SuspendLeaseResponse) -> Result<SuspendLeaseOutput, ToolError> {
    match response {
        SuspendLeaseResponse::Suspended { level, reap_at, .. } => Ok(SuspendLeaseOutput {
            outcome: "suspended".into(),
            level: level.as_str().into(),
            reap_at,
        }),
        SuspendLeaseResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

pub(super) fn wake_output(response: WakeLeaseResponse) -> Result<WakeLeaseOutput, ToolError> {
    match response {
        WakeLeaseResponse::Woken { deadline, .. } => Ok(WakeLeaseOutput {
            outcome: "woken".into(),
            deadline,
        }),
        WakeLeaseResponse::Denied(e) => Err(ToolError::denied(e)),
    }
}

#[cfg(test)]
mod tests;
