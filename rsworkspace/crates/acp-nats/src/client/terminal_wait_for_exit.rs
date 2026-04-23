use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, Request, Response, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use bytes::Bytes;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum TerminalWaitForExitError {
    MalformedJson(serde_json::Error),
    InvalidParams(agent_client_protocol::Error),
    TimedOut,
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for TerminalWaitForExitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson(e) => write!(f, "malformed JSON: {}", e),
            Self::InvalidParams(e) => write!(f, "invalid params: {}", e),
            Self::TimedOut => write!(f, "Timed out waiting for terminal exit"),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for TerminalWaitForExitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MalformedJson(e) => Some(e),
            Self::InvalidParams(e) => Some(e),
            Self::TimedOut => None,
            Self::ClientError(e) => Some(e),
        }
    }
}

fn invalid_params_error(message: impl Into<String>) -> TerminalWaitForExitError {
    TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(
        i32::from(ErrorCode::InvalidParams),
        message.into(),
    ))
}

pub fn error_code_and_message(e: &TerminalWaitForExitError) -> (ErrorCode, String) {
    match e {
        TerminalWaitForExitError::MalformedJson(inner) => (
            ErrorCode::ParseError,
            format!("Malformed terminal/wait_for_exit request JSON: {}", inner),
        ),
        TerminalWaitForExitError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalWaitForExitError::TimedOut => (
            ErrorCode::InternalError,
            "Timed out waiting for terminal exit".to_string(),
        ),
        TerminalWaitForExitError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(
    name = "acp.client.terminal.wait_for_exit",
    skip(payload, client, nats, serializer),
    fields(session_id = %expected_session_id)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
    operation_timeout: Duration,
    serializer: &S,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %expected_session_id,
                "terminal/wait_for_exit requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, expected_session_id, operation_timeout).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(
                        error = %e,
                        "JSON serialization of response failed, sending error reply"
                    );
                    rpc_reply::error_response_bytes(
                        serializer,
                        request_id,
                        ErrorCode::InternalError,
                        &format!("Failed to serialize response: {}", e),
                    )
                });
            rpc_reply::publish_reply(
                nats,
                reply_to,
                response_bytes,
                content_type,
                "terminal/wait_for_exit reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/wait_for_exit"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal/wait_for_exit error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
    operation_timeout: Duration,
) -> Result<WaitForTerminalExitResponse, TerminalWaitForExitError> {
    let request = parse_request(payload, expected_session_id)?;
    timeout(operation_timeout, client.wait_for_terminal_exit(request))
        .await
        .map_err(|_| TerminalWaitForExitError::TimedOut)?
        .map_err(TerminalWaitForExitError::ClientError)
}

fn parse_request(
    payload: &[u8],
    expected_session_id: &str,
) -> Result<WaitForTerminalExitRequest, TerminalWaitForExitError> {
    let payload_value: serde_json::Value =
        serde_json::from_slice(payload).map_err(TerminalWaitForExitError::MalformedJson)?;
    let envelope: Request<WaitForTerminalExitRequest> = serde_json::from_value(payload_value)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/wait_for_exit request: {}", e)))?;
    let request = envelope
        .params
        .ok_or_else(|| invalid_params_error("params is null or missing"))?;

    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(invalid_params_error(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        )));
    }

    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{MockClient, TerminalWaitForExitFailingClient, TerminalWaitForExitTimeoutClient};
    use super::*;
    use agent_client_protocol::{Request, RequestId, WaitForTerminalExitRequest};
    use std::error::Error;
    use std::time::Duration;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    #[tokio::test]
    async fn handle_success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let parsed: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(parsed.get("id"), Some(&serde_json::Value::from(1)));
        assert!(parsed.get("result").is_some());
    }

    #[tokio::test]
    async fn handle_no_reply_does_not_call_client_or_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            None,
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
        assert_eq!(client.wait_for_terminal_exit_call_count(), 0);
    }

    #[tokio::test]
    async fn handle_malformed_json_publishes_parse_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();

        handle(
            b"not json",
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32700))
        );
    }

    #[tokio::test]
    async fn handle_invalid_params_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let payload = br#"{"id":1,"method":"terminal/wait_for_exit","params":{}}"#;

        handle(
            payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_params_null_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let payload = br#"{"id":1,"method":"terminal/wait_for_exit","params":null}"#;

        handle(
            payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-b", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-a",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        assert_eq!(client.wait_for_terminal_exit_call_count(), 0);
    }

    #[tokio::test]
    async fn handle_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert!(
            response
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.contains("mock wait_for_terminal_exit failure"))
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn handle_timeout_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            Duration::from_millis(10),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let serializer = FailNextSerialize::new(1);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &serializer,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_success_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_success_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            Duration::from_secs(5),
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[test]
    fn error_code_and_message_malformed_json() {
        let err = TerminalWaitForExitError::MalformedJson(serde_json::from_str::<serde_json::Value>("{").unwrap_err());
        let (code, msg) = error_code_and_message(&err);
        assert_eq!(code, ErrorCode::ParseError);
        assert!(msg.contains("Malformed terminal/wait_for_exit"));
    }

    #[test]
    fn error_code_and_message_invalid_params() {
        let err = TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
        let (code, msg) = error_code_and_message(&err);
        assert_eq!(i32::from(code), -32602);
        assert_eq!(msg, "bad params");
    }

    #[test]
    fn error_code_and_message_timed_out() {
        let err = TerminalWaitForExitError::TimedOut;
        let (code, msg) = error_code_and_message(&err);
        assert_eq!(code, ErrorCode::InternalError);
        assert_eq!(msg, "Timed out waiting for terminal exit");
    }

    #[test]
    fn error_code_and_message_client_error() {
        let err = TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
        let (code, msg) = error_code_and_message(&err);
        assert_eq!(i32::from(code), -32603);
        assert_eq!(msg, "client failed");
    }

    #[test]
    fn terminal_wait_for_exit_error_display() {
        let err = TerminalWaitForExitError::TimedOut;
        assert!(err.to_string().contains("Timed out"));
        let json_err = TerminalWaitForExitError::MalformedJson(serde_json::from_str::<()>("{").unwrap_err());
        assert!(json_err.to_string().contains("malformed JSON"));
        let params_err =
            TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
        assert!(params_err.to_string().contains("invalid params"));
        let client_err =
            TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
        assert!(client_err.to_string().contains("client error"));
    }

    #[test]
    fn terminal_wait_for_exit_error_source() {
        let err = TerminalWaitForExitError::TimedOut;
        assert!(err.source().is_none());
        let json_err = TerminalWaitForExitError::MalformedJson(serde_json::from_str::<()>("{").unwrap_err());
        assert!(json_err.source().is_some());
        let params_err =
            TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(-32602, "bad params"));
        assert!(params_err.source().is_some());
        let client_err =
            TerminalWaitForExitError::ClientError(agent_client_protocol::Error::new(-32603, "client failed"));
        assert!(client_err.source().is_some());
    }
}
