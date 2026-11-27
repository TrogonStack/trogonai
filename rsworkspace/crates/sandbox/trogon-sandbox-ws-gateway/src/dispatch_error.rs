use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{TerminalError, TerminalIdError};
use trogon_sandbox_protocol::{Classified, ErrorKind};

use crate::{BackendError, EntropyError, PayloadError, PreviewGrantError, WsErrorBody, WsFrame, client_error_frame};

/// Why a tool call did not produce a result.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DispatchError {
    #[error(transparent)]
    Invalid(#[from] PayloadError),
    #[error(transparent)]
    Client(Box<ClientError>),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("lease tenant must match session tenant")]
    TenantMismatch,
    #[error("response could not be serialized: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    PreviewGrant(#[from] PreviewGrantError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error("the provider does not support interactive terminals")]
    TerminalUnsupported,
    #[error(transparent)]
    Entropy(#[from] EntropyError),
    #[error("port probe client could not be built: {0}")]
    Probe(#[from] reqwest::Error),
    #[error("generated terminal id is invalid: {0}")]
    TerminalId(#[from] TerminalIdError),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

impl From<ClientError> for DispatchError {
    fn from(error: ClientError) -> Self {
        Self::Client(Box::new(error))
    }
}

impl DispatchError {
    /// The error response frame for request `id`.
    pub(crate) fn into_frame(self, id: String) -> WsFrame {
        match self {
            Self::Invalid(error) => WsFrame::response_err(id, "invalid_payload", error.to_string()),
            Self::Client(error) => client_error_frame(id, &error),
            Self::Backend(error) => WsFrame::response_err(id, "backend", error.to_string()),
            Self::TenantMismatch => WsFrame::response_err(id, "tenant_mismatch", self.to_string()),
            Self::Serialize(error) => WsFrame::response_err(id, "internal", error.to_string()),
            Self::PreviewGrant(error) => WsFrame::response_err(id, "preview_grant", error.to_string()),
            Self::Terminal(error) => terminal_error_frame(id, &error),
            Self::TerminalUnsupported => unsupported_frame(id),
            Self::Entropy(_) | Self::Probe(_) | Self::TerminalId(_) => {
                WsFrame::response_err(id, "internal", self.to_string())
            }
            Self::UnknownTool(_) => WsFrame::response_err(id, "unknown_tool", self.to_string()),
        }
    }
}

fn terminal_error_frame(id: String, error: &TerminalError) -> WsFrame {
    let class = error.class();
    let mut body = WsErrorBody::new("terminal_failed", error.to_string());
    body.kind = Some(error.kind().to_string());
    body.retryable = Some(class.retryable);
    body.outcome_unknown = Some(class.outcome_unknown);
    body.hint = Some(class.hint.to_string());
    WsFrame::response_error_body(id, body)
}

fn unsupported_frame(id: String) -> WsFrame {
    let class = trogon_sandbox_protocol::ErrorClass::permanent(
        "the provider does not support interactive terminals; use a provider that does",
    );
    let mut body = WsErrorBody::new("unsupported", "the provider does not support interactive terminals");
    body.retryable = Some(class.retryable);
    body.outcome_unknown = Some(class.outcome_unknown);
    body.hint = Some(class.hint.to_string());
    WsFrame::response_error_body(id, body)
}

#[cfg(test)]
mod tests;
