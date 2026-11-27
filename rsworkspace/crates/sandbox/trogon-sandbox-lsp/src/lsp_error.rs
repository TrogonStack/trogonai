use std::time::Duration;

use serde_json::Value;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::StdinChunkError;

/// Why a language server call failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LspError {
    #[error("LSP protocol error: {0}")]
    Protocol(String),
    #[error("LSP request {method} timed out after {timeout:?}")]
    RequestTimeout { method: String, timeout: Duration },
    #[error("language server is not running: {0}")]
    NotRunning(String),
    #[error("language server exited{}", display_exit_code(*.0))]
    ServerExited(Option<i32>),
    #[error("LSP response error {code}: {message}")]
    Response {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("sandbox transport: {0}")]
    Transport(String),
    #[error("stdin: {0}")]
    Stdin(String),
    #[error("stdin chunk: {0}")]
    StdinChunk(#[from] StdinChunkError),
}

fn display_exit_code(code: Option<i32>) -> String {
    code.map(|c| format!(" with code {c}")).unwrap_or_default()
}

impl From<ClientError> for LspError {
    fn from(error: ClientError) -> Self {
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for LspError {
    fn from(error: serde_json::Error) -> Self {
        Self::Protocol(format!("JSON encoding failed: {error}"))
    }
}
