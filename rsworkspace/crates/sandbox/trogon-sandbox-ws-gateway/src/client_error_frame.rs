use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::LeaseError;
use trogon_sandbox_protocol::{ErrorKind, LeaseErrorSubject};

use crate::{WsErrorBody, WsFrame};

/// The stable `code` and a message for a broker client failure.
pub fn client_error_code(error: &ClientError) -> (&'static str, String) {
    match error {
        ClientError::Denied(denial) => ("lease_denied", format!("{denial:?}")),
        ClientError::Timeout(waited) => ("timeout", format!("broker timeout after {waited:?}")),
        ClientError::Transport(failure) => ("transport", failure.to_string()),
        ClientError::DriveDenied(denial) => ("drive_denied", format!("{denial:?}")),
        ClientError::FileOpUnsupported(message) => ("unsupported", message.clone()),
        ClientError::FileOpFailed { kind, message } => ("file_op_failed", format!("{kind:?}: {message}")),
        ClientError::FileNotFound(path) => ("not_found", path.clone()),
        ClientError::EditRejected { .. } => ("edit_rejected", error.to_string()),
    }
}

/// The error response frame for a broker client failure: the stable `code` and message plus the machine-readable
/// classification, so clients can decide on a retry without carrying a table of broker variants.
pub fn client_error_frame(id: impl Into<String>, error: &ClientError) -> WsFrame {
    let (code, message) = client_error_code(error);
    let class = error.class();
    let mut body = WsErrorBody::new(code, message);
    body.retryable = Some(class.retryable);
    body.outcome_unknown = Some(class.outcome_unknown);
    body.hint = Some(class.hint.to_string());
    match error {
        ClientError::Denied(denial) => {
            body.kind = Some(denial.kind().to_string());
            body.provider = denial
                .provider()
                .and_then(|provider| serde_json::to_value(provider).ok())
                .and_then(|value| value.as_str().map(str::to_string));
            body.lease_id = denial.lease_id().map(|lease| lease.as_str().to_string());
            if let LeaseError::LeaseWaking { retry_after_ms, .. } = denial {
                body.retry_after_ms = Some(*retry_after_ms);
            }
        }
        ClientError::DriveDenied(denial) => body.kind = Some(denial.kind().to_string()),
        ClientError::FileOpFailed { kind, .. } => {
            body.kind = serde_json::to_value(kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string));
        }
        _ => {}
    }
    WsFrame::response_error_body(id, body)
}
