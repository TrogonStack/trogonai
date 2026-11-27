use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{LeaseError, TerminalError, TerminalId, TerminalResponse};

use super::terminal_outcome;
use crate::tool_error::ToolError;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn opened_and_done_succeed() -> TestResult {
    terminal_outcome(TerminalResponse::Opened {
        terminal_id: TerminalId::new("t1")?,
    })?;
    terminal_outcome(TerminalResponse::Done)?;
    Ok(())
}

#[test]
fn a_failure_keeps_its_cause() {
    let result = terminal_outcome(TerminalResponse::Failed(TerminalError::Exited));
    assert!(matches!(result, Err(ToolError::Terminal(TerminalError::Exited))));
}

#[test]
fn an_unsupported_provider_is_reported() {
    let result = terminal_outcome(TerminalResponse::Unsupported);
    assert!(matches!(result, Err(ToolError::TerminalUnsupported)));
}

#[test]
fn a_denial_is_a_client_error() {
    let result = terminal_outcome(TerminalResponse::Denied(LeaseError::NoProviderAvailable));
    assert!(matches!(result, Err(ToolError::Client(e)) if matches!(*e, ClientError::Denied(_))));
}
