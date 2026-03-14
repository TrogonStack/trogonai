use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{Client, ErrorCode, Request, Response, TerminalOutputRequest};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[instrument(
    name = "acp.client.terminal.output",
    skip(payload, client, nats, serializer)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    session_id: &str,
    serializer: &S,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %session_id,
                "terminal/output requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);

    let request = match parse_request(payload, session_id) {
        Ok(req) => req,
        Err((code, message)) => {
            warn!(
                error = %message,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            let (bytes, content_type) =
                rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal/output error reply",
            )
            .await;
            return;
        }
    };

    match client.terminal_output(request).await {
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
                "terminal/output reply",
            )
            .await;
        }
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            let (bytes, content_type) =
                rpc_reply::error_response_bytes(serializer, request_id, e.code, &e.message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal/output error reply",
            )
            .await;
        }
    }
}

fn parse_request(
    payload: &[u8],
    expected_session_id: &str,
) -> Result<TerminalOutputRequest, (ErrorCode, String)> {
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|e| (ErrorCode::ParseError, format!("Malformed JSON: {}", e)))?;

    let envelope: Request<TerminalOutputRequest> = serde_json::from_value(value)
        .map_err(|e| (ErrorCode::InvalidParams, format!("Invalid request: {}", e)))?;

    let request = envelope.params.ok_or_else(|| {
        (
            ErrorCode::InvalidParams,
            "params is null or missing".to_string(),
        )
    })?;

    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err((
            ErrorCode::InvalidParams,
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        ));
    }

    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        RequestId, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, TerminalOutputResponse,
    };
    use async_trait::async_trait;
    use std::sync::Arc;
    use trogon_nats::MockNatsClient;
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    struct MockClient {
        terminal_output_result: agent_client_protocol::Result<TerminalOutputResponse>,
    }

    impl MockClient {
        fn success() -> Self {
            Self {
                terminal_output_result: Ok(TerminalOutputResponse::new("output", false)),
            }
        }

        fn failing() -> Self {
            Self {
                terminal_output_result: Err(agent_client_protocol::Error::new(
                    -32603,
                    "mock failure",
                )),
            }
        }
    }

    #[async_trait(?Send)]
    impl Client for MockClient {
        async fn session_notification(
            &self,
            _: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            Ok(())
        }

        async fn request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            self.terminal_output_result.clone()
        }
    }

    fn envelope_payload() -> Vec<u8> {
        let request = TerminalOutputRequest::new("sess-1", "term-001");
        let envelope = Request {
            id: RequestId::Number(1),
            method: Arc::from("terminal/output"),
            params: Some(request),
        };
        serde_json::to_vec(&envelope).unwrap()
    }

    #[tokio::test]
    async fn success_publishes_response_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let payload = envelope_payload();

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

    #[tokio::test]
    async fn no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let payload = envelope_payload();

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn malformed_json_publishes_parse_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();

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
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32700)),
        );
    }

    #[tokio::test]
    async fn invalid_params_publishes_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let payload = br#"{"id":1,"method":"terminal/output","params":{}}"#;

        handle(
            payload.as_slice(),
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32602)),
        );
    }

    #[tokio::test]
    async fn null_params_publishes_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let payload = br#"{"id":1,"method":"terminal/output","params":null}"#;

        handle(
            payload.as_slice(),
            &client,
            Some("_INBOX.err"),
            &nats,
            "sess-1",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32602)),
        );
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains("params is null"));
    }

    #[tokio::test]
    async fn session_id_mismatch_publishes_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let payload = envelope_payload();

        handle(
            &payload,
            &client,
            Some("_INBOX.err"),
            &nats,
            "different-session",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32602)),
        );
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains("does not match"));
    }

    #[tokio::test]
    async fn client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = MockClient::failing();
        let payload = envelope_payload();

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
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
    }

    #[tokio::test]
    async fn serialization_failure_sends_fallback_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::success();
        let serializer = FailNextSerialize::new(1);
        let payload = envelope_payload();

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
    async fn mock_client_trait_methods_exercise_coverage() {
        use agent_client_protocol::{
            ContentBlock, ContentChunk, SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
        };

        let client = MockClient::success();
        let notification = SessionNotification::new(
            "sess-1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
        );
        assert!(client.session_notification(notification).await.is_ok());

        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let req = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        assert!(client.request_permission(req).await.is_ok());
    }
}
