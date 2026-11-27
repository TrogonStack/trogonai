use std::error::Error;

use rmcp::model::IntoContents;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{LeaseError, TerminalError, TerminalId};

use crate::policy::{CommandVetoError, PathVetoError};

use super::ToolError;

#[test]
fn broker_errors_carry_their_classification() {
    let text = ToolError::denied(LeaseError::NoProviderAvailable).text();
    assert!(text.contains("retryable: true"), "{text}");
    assert!(text.contains("outcome_unknown: false"), "{text}");
    assert!(text.contains("hint: "), "{text}");
}

#[test]
fn other_errors_keep_their_plain_message() {
    let error = ToolError::InvalidChoice {
        field: "level",
        got: "x".to_string(),
        expected: "auto, memory or disk",
    };
    assert_eq!(error.text(), "invalid level: got x, expected auto, memory or disk");
}

#[test]
fn invalid_input_names_the_field_and_keeps_the_cause() -> Result<(), Box<dyn Error>> {
    let cause = trogon_sandbox_contract::TenantId::new("")
        .err()
        .ok_or("expected an error")?;
    let error = ToolError::invalid("tenant", cause);
    assert!(error.text().starts_with("invalid tenant: "));
    assert!(error.source().is_some());
    Ok(())
}

#[test]
fn every_variant_renders_a_message() -> Result<(), Box<dyn Error>> {
    let errors = [
        ToolError::Unauthenticated,
        ToolError::ToolDenied("sandbox_x".to_string()),
        ToolError::NotConnected,
        ToolError::InvalidCombination {
            option: "process",
            requires: "detached=true",
        },
        ToolError::ExecutionUnknown("e1".to_string()),
        ToolError::ExecutionUnfinished("e1".to_string()),
        ToolError::UnexpectedReply("stat"),
        ToolError::Terminal(TerminalError::Exited),
        ToolError::TerminalUnsupported,
        ToolError::TerminalUnderCommandPolicy,
        ToolError::TerminalNotOpen("t1".to_string()),
        ToolError::from(ClientError::Timeout(std::time::Duration::from_secs(1))),
        ToolError::CommandDenied(CommandVetoError::NotAllowed),
        ToolError::PathDenied(PathVetoError::Empty),
        ToolError::Encode(serde_json::from_str::<u8>("x").err().ok_or("expected an error")?),
        ToolError::TerminalId(TerminalId::new("").err().ok_or("expected an error")?),
    ];
    for error in errors {
        assert!(!error.text().is_empty());
    }
    Ok(())
}

#[test]
fn contents_hold_one_text_block() {
    assert_eq!(ToolError::NotConnected.into_contents().len(), 1);
}
