use serde_json::Value;
use trogon_sandbox_contract::{
    Base64Data, CommandOutput, ExecutionId, FileOpResponse, PollCommandResponse, ProviderLease, ReadFileResponse,
    RestartCommandResponse, RunCommandResponse,
};

use crate::CheckFailureError;
use crate::encoding::text;

#[cfg(test)]
mod tests;

/// The grant in a dispatch reply, or the reply that was not one.
pub(crate) fn granted(reply: Value, expected: &str) -> Result<ProviderLease, CheckFailureError> {
    serde_json::from_value(reply.clone()).map_err(|_| CheckFailureError::unexpected(expected, &reply))
}

/// The output of a command that ran to completion.
pub(crate) fn completed(response: RunCommandResponse) -> Result<CommandOutput, CheckFailureError> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        other => Err(CheckFailureError::unexpected("a completed command", &other)),
    }
}

/// The execution id of a detached command that started.
pub(crate) fn started(response: RunCommandResponse) -> Result<ExecutionId, CheckFailureError> {
    match response {
        RunCommandResponse::Started(started) => Ok(started.execution_id),
        other => Err(CheckFailureError::unexpected("a started command", &other)),
    }
}

/// The execution id a restart moved the command to.
pub(crate) fn restarted(response: RestartCommandResponse) -> Result<ExecutionId, CheckFailureError> {
    match response {
        RestartCommandResponse::Restarted { execution_id } => Ok(execution_id),
        other => Err(CheckFailureError::unexpected("a restarted command", &other)),
    }
}

/// `Some` once the polled command completed, `None` while it still runs.
pub(crate) fn finished(response: PollCommandResponse) -> Result<Option<CommandOutput>, CheckFailureError> {
    match response {
        PollCommandResponse::Completed(output) => Ok(Some(output)),
        PollCommandResponse::Running => Ok(None),
        other => Err(CheckFailureError::unexpected("a running or completed command", &other)),
    }
}

/// The bytes a file read returned.
pub(crate) fn content(response: FileOpResponse, path: &str) -> Result<Base64Data, CheckFailureError> {
    match response {
        FileOpResponse::Content { data, .. } => Ok(data),
        other => Err(CheckFailureError::unexpected(format!("the content of {path}"), &other)),
    }
}

/// The state a JS runtime kept, `None` when the lease refuses to answer.
pub(crate) fn state_from_eval(response: RunCommandResponse) -> Result<Option<Vec<u8>>, CheckFailureError> {
    match response {
        RunCommandResponse::Completed(out) if out.exit_code == 0 => {
            Ok(Some(text(&out.stdout).trim_end().as_bytes().to_vec()))
        }
        RunCommandResponse::Denied(_) => Ok(None),
        other => Err(CheckFailureError::unexpected("the kept state or a refusal", &other)),
    }
}

/// The state a shell lease kept, `None` when the lease says it has nothing there.
pub(crate) fn state_from_file(response: ReadFileResponse) -> Result<Option<Vec<u8>>, CheckFailureError> {
    match response {
        ReadFileResponse::Content { data } => Ok(Some(data)),
        ReadFileResponse::NotFound | ReadFileResponse::Denied(_) => Ok(None),
    }
}

/// The packed tree an archive read returned.
pub(crate) fn archive(response: FileOpResponse) -> Result<Base64Data, CheckFailureError> {
    match response {
        FileOpResponse::Archive { data, entries, .. } if entries > 0 => Ok(data),
        other => Err(CheckFailureError::unexpected("an archive with entries", &other)),
    }
}

/// Whether the reply unpacked at least one entry.
pub(crate) fn extracted(response: &FileOpResponse) -> bool {
    matches!(response, FileOpResponse::Extracted { entries, .. } if *entries > 0)
}
