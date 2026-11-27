use trogon_sandbox_contract::TerminalResponse;

use crate::tool_error::ToolError;

/// Turns the broker's answer to one terminal operation into success or the error the model sees.
pub(super) fn terminal_outcome(response: TerminalResponse) -> Result<(), ToolError> {
    match response {
        TerminalResponse::Opened { .. } | TerminalResponse::Done => Ok(()),
        TerminalResponse::Failed(error) => Err(ToolError::Terminal(error)),
        TerminalResponse::Unsupported => Err(ToolError::TerminalUnsupported),
        TerminalResponse::Denied(error) => Err(ToolError::denied(error)),
    }
}

#[cfg(test)]
mod tests;
