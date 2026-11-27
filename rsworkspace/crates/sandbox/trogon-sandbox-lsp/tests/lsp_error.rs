use std::time::Duration;

use trogon_sandbox_client::{BusError, ClientError, TransportError};
use trogon_sandbox_contract::StdinChunkError;
use trogon_sandbox_lsp::LspError;

#[test]
fn a_server_exit_reports_its_code_when_known() {
    assert_eq!(
        LspError::ServerExited(Some(3)).to_string(),
        "language server exited with code 3"
    );
    assert_eq!(LspError::ServerExited(None).to_string(), "language server exited");
}

#[test]
fn a_timeout_names_the_method() {
    let err = LspError::RequestTimeout {
        method: "textDocument/hover".into(),
        timeout: Duration::from_millis(250),
    };
    let text = format!("{err}");
    assert!(text.contains("textDocument/hover"));
}

#[test]
fn a_client_error_becomes_a_transport_error() {
    let err = LspError::from(ClientError::Timeout(Duration::from_secs(1)));
    assert!(matches!(err, LspError::Transport(m) if m.contains("timeout")));
    let bus = ClientError::Transport(TransportError::from(BusError::Custom("down".into())));
    assert!(matches!(LspError::from(bus), LspError::Transport(_)));
}

#[test]
fn a_json_failure_becomes_a_protocol_error() -> Result<(), Box<dyn std::error::Error>> {
    let source = serde_json::from_str::<u8>("x").err().ok_or("x parsed as a number")?;
    assert!(matches!(LspError::from(source), LspError::Protocol(m) if m.contains("JSON")));
    Ok(())
}

#[test]
fn a_stdin_chunk_failure_is_wrapped() {
    let err = LspError::from(StdinChunkError::TooLarge { max: 4 });
    assert!(matches!(err, LspError::StdinChunk(_)));
    let text = format!("{err}");
    assert!(text.starts_with("stdin chunk"));
}
