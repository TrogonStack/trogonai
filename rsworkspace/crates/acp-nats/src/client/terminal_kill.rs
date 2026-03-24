use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, KillTerminalRequest, KillTerminalResponse, Request, Response,
};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug)]
pub enum TerminalKillError {
    MalformedJson(serde_json::Error),
    InvalidParams(agent_client_protocol::Error),
    ClientError(agent_client_protocol::Error),
}

impl std::fmt::Display for TerminalKillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson(e) => write!(f, "malformed JSON: {}", e),
            Self::InvalidParams(e) => write!(f, "invalid params: {}", e),
            Self::ClientError(e) => write!(f, "client error: {}", e),
        }
    }
}

impl std::error::Error for TerminalKillError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MalformedJson(e) => Some(e),
            Self::InvalidParams(e) => Some(e),
            Self::ClientError(e) => Some(e),
        }
    }
}

fn invalid_params_error(message: impl Into<String>) -> TerminalKillError {
    TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
        i32::from(ErrorCode::InvalidParams),
        message.into(),
    ))
}

pub fn error_code_and_message(e: &TerminalKillError) -> (ErrorCode, String) {
    match e {
        TerminalKillError::MalformedJson(inner) => (
            ErrorCode::ParseError,
            format!("Malformed terminal/kill request JSON: {}", inner),
        ),
        TerminalKillError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalKillError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(
    name = "acp.client.terminal.kill",
    skip(payload, client, nats, serializer)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
    serializer: &S,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %expected_session_id,
                "terminal/kill requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, expected_session_id).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(error = %e, "JSON serialization of response failed, sending error reply");
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
                "terminal_kill reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/kill"
            );
            let (bytes, content_type) =
                rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal_kill error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<KillTerminalResponse, TerminalKillError> {
    let payload_value: serde_json::Value =
        serde_json::from_slice(payload).map_err(TerminalKillError::MalformedJson)?;
    let envelope: Request<KillTerminalRequest> = serde_json::from_value(payload_value)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/kill request: {}", e)))?;
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

    client
        .kill_terminal(request)
        .await
        .map_err(TerminalKillError::ClientError)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{MockClient, TerminalKillFailingClient};
    use super::*;
    use std::error::Error;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    #[tokio::test]
    async fn handle_success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
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
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
        assert_eq!(client.kill_terminal_call_count(), 0);
    }

    #[tokio::test]
    async fn handle_invalid_json_publishes_parse_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();

        handle(
            b"not json",
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32700))
        );
    }

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-b"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-a",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn handle_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("message")),
            Some(&serde_json::Value::from("mock kill_terminal failure"))
        );
    }

    #[tokio::test]
    async fn handle_success_serialization_fallback_sends_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let serializer = FailNextSerialize::new(1);
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
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
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
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
        let request = KillTerminalRequest::new(
            agent_client_protocol::SessionId::from("sess-1"),
            "term-001".to_string(),
        );
        let envelope = Request {
            id: agent_client_protocol::RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(request),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[test]
    fn error_code_and_message_client_error_preserves_client_code() {
        let err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "bad terminal id",
        ));
        let (code, message) = error_code_and_message(&err);
        assert_eq!(code, ErrorCode::InvalidParams);
        assert_eq!(message, "bad terminal id");
    }

    #[test]
    fn error_code_and_message_malformed_json_returns_parse_error() {
        let err = TerminalKillError::MalformedJson(
            serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err(),
        );
        let (code, message) = error_code_and_message(&err);
        assert_eq!(code, ErrorCode::ParseError);
        assert!(message.contains("Malformed terminal/kill request JSON"));
    }

    #[test]
    fn terminal_kill_error_display() {
        let malformed = TerminalKillError::MalformedJson(
            serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err(),
        );
        assert!(malformed.to_string().contains("malformed JSON"));

        let invalid_params = TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "bad params",
        ));
        assert!(invalid_params.to_string().contains("invalid params"));

        let client_err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "client fail",
        ));
        assert!(client_err.to_string().contains("client error"));
    }

    #[test]
    fn terminal_kill_error_source() {
        let malformed = TerminalKillError::MalformedJson(
            serde_json::from_slice::<serde_json::Value>(b"not json").unwrap_err(),
        );
        assert!(malformed.source().is_some());

        let invalid_params = TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "bad params",
        ));
        assert!(invalid_params.source().is_some());

        let client_err = TerminalKillError::ClientError(agent_client_protocol::Error::new(
            ErrorCode::InvalidParams.into(),
            "client fail",
        ));
        assert!(client_err.source().is_some());
    }
}
