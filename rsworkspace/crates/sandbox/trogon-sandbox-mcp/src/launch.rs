//! Turns the plain `sandbox_run_command` input into a typed launch request.

use std::num::NonZeroU16;

use trogon_sandbox_contract::{
    CommandDeadline, CommandEnv, CommandExecution, LaunchCancellation, LaunchKey, OutputRetention, ProcessName,
    ProcessSpec, ReadinessNeedle, ReadinessProbe, RedactValue, RedactValues, RunCommandRequest, StdinMode,
    StreamedOutput,
};

use crate::inputs::command_inputs::{
    CommandDeadlineInput, OutputRetentionInput, ProcessInput, ReadinessInput, RunCommandInput, StdinModeInput,
};
use crate::parse;
use crate::tool_error::ToolError;

/// How the caller wants to wait for the command.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum LaunchMode {
    /// Block until the command finishes.
    Attached,
    /// Return the execution id at once.
    Detached,
    /// Run detached with streaming on, report the output as progress, then collect the result.
    Progress,
}

/// A validated launch: the request to send and how to wait for it.
#[derive(Debug)]
pub(crate) struct LaunchPlan {
    pub request: RunCommandRequest,
    pub mode: LaunchMode,
}

/// `listening` says whether the caller sent a progress token, so `stream_progress` has somewhere to report.
pub(crate) fn plan(input: RunCommandInput, listening: bool) -> Result<LaunchPlan, ToolError> {
    let detached = input.detached.unwrap_or(false);
    let wants_progress = input.stream_progress.unwrap_or(false);
    let progress = wants_progress && listening;
    let deadline = match input.deadline {
        None | Some(CommandDeadlineInput::Timeout) => CommandDeadline::Timeout,
        Some(CommandDeadlineInput::Lease) => CommandDeadline::Lease,
    };
    let stdin = match input.stdin {
        None | Some(StdinModeInput::Closed) => StdinMode::Closed,
        Some(StdinModeInput::Interactive) => StdinMode::Interactive,
    };
    let output_retention = match input.output_retention {
        None | Some(OutputRetentionInput::Buffer) => OutputRetention::Buffer,
        Some(OutputRetentionInput::StreamOnly) => OutputRetention::StreamOnly,
    };
    if wants_progress && detached {
        return Err(ToolError::InvalidCombination {
            option: "stream_progress",
            requires: "detached=false",
        });
    }
    if !detached {
        require_detached(deadline == CommandDeadline::Lease, "deadline lease")?;
        require_detached(stdin == StdinMode::Interactive, "stdin interactive")?;
        require_detached(
            output_retention == OutputRetention::StreamOnly,
            "output_retention stream_only",
        )?;
        require_detached(input.process.is_some(), "process")?;
    }
    let process = input.process.map(process_spec).transpose()?;
    let launch_key = input
        .launch_key
        .map(|key| LaunchKey::new(key).map_err(|e| ToolError::invalid("launch_key", e)))
        .transpose()?;
    let redact_values = RedactValues::new(input.redact_values.into_iter().map(|v| RedactValue::new(v.0)).collect())
        .map_err(|e| ToolError::invalid("redact_values", e))?;
    let (execution, mode, stream_output, retention) = if detached {
        (
            CommandExecution::Detached {
                stdin,
                deadline,
                process,
            },
            LaunchMode::Detached,
            StreamedOutput::from(input.stream_output.unwrap_or(false)),
            output_retention,
        )
    } else if progress {
        (
            CommandExecution::Detached {
                stdin: StdinMode::Closed,
                deadline: CommandDeadline::Timeout,
                process: None,
            },
            LaunchMode::Progress,
            StreamedOutput::Enabled,
            OutputRetention::Buffer,
        )
    } else {
        (
            CommandExecution::Attached,
            LaunchMode::Attached,
            StreamedOutput::from(input.stream_output.unwrap_or(false)),
            OutputRetention::Buffer,
        )
    };
    let request = RunCommandRequest {
        lease_id: parse::lease_id(input.lease_id),
        command: input.command,
        cwd: input.cwd,
        env: CommandEnv::new(input.env).map_err(|e| ToolError::invalid("env", e))?,
        timeout_seconds: input.timeout_seconds,
        stream_output,
        run_as_user: parse::optional_username(input.user)?,
        output_retention: retention,
        max_output_bytes: None,
        redact_values,
        launch_key,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution,
    };
    Ok(LaunchPlan { request, mode })
}

fn require_detached(offending: bool, option: &'static str) -> Result<(), ToolError> {
    if offending {
        return Err(ToolError::InvalidCombination {
            option,
            requires: "detached=true",
        });
    }
    Ok(())
}

fn process_spec(input: ProcessInput) -> Result<ProcessSpec, ToolError> {
    Ok(ProcessSpec {
        name: input
            .name
            .map(|name| ProcessName::new(name).map_err(|e| ToolError::invalid("process.name", e)))
            .transpose()?,
        readiness: input.readiness.map(readiness_probe).transpose()?,
    })
}

fn readiness_probe(input: ReadinessInput) -> Result<ReadinessProbe, ToolError> {
    match input {
        ReadinessInput::TcpPort { port } => NonZeroU16::new(port)
            .map(|port| ReadinessProbe::TcpPort { port })
            .ok_or_else(|| ToolError::InvalidChoice {
                field: "process.readiness.port",
                got: port.to_string(),
                expected: "a port from 1 to 65535",
            }),
        ReadinessInput::OutputContains { needle } => ReadinessNeedle::new(needle)
            .map(|needle| ReadinessProbe::OutputContains { needle })
            .map_err(|e| ToolError::invalid("process.readiness.needle", e)),
    }
}

#[cfg(test)]
mod tests;
