use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{ElicitationAction, ElicitationRequest, ElicitationResponse};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

/// Implemented by anything that can field an elicitation request from the agent.
/// The default returns `Cancel`, which is correct for bridges that do not yet
/// support forwarding elicitations to the real client.
#[async_trait::async_trait(?Send)]
pub trait ElicitationClient {
    async fn request_elicitation(
        &self,
        _request: ElicitationRequest,
    ) -> agent_client_protocol::Result<ElicitationResponse> {
        Ok(ElicitationResponse::new(ElicitationAction::Cancel))
    }
}

impl<T> ElicitationClient for T {}

/// Handles `session.elicitation`: parses raw JSON request, calls client, publishes raw JSON
/// response to reply subject. On any error the handler publishes a `Cancel` response.
#[instrument(
    name = "acp.client.session.elicitation",
    skip(payload, client, nats, serializer)
)]
pub async fn handle<N: PublishClient + FlushClient, C: ElicitationClient, S: JsonSerialize>(
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
                "session.elicitation requires reply subject; ignoring message"
            );
            return;
        }
    };

    let cancelled = ElicitationResponse::new(ElicitationAction::Cancel);

    let request: ElicitationRequest = match serde_json::from_slice(payload) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id,
                "invalid elicitation payload — replying Cancel"
            );
            let (bytes, ct) = serializer
                .to_vec(&cancelled)
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|_| (Bytes::new(), rpc_reply::CONTENT_TYPE_JSON));
            rpc_reply::publish_reply(nats, reply_to, bytes, ct, "elicitation parse error reply").await;
            return;
        }
    };

    if request.session_id.to_string() != session_id {
        warn!(
            params_session_id = %request.session_id,
            subject_session_id = %session_id,
            "session_id mismatch in elicitation — replying Cancel"
        );
        let (bytes, ct) = serializer
            .to_vec(&cancelled)
            .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
            .unwrap_or_else(|_| (Bytes::new(), rpc_reply::CONTENT_TYPE_JSON));
        rpc_reply::publish_reply(nats, reply_to, bytes, ct, "elicitation session_id mismatch reply").await;
        return;
    }

    let response = match client.request_elicitation(request).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id,
                "elicitation client error — replying Cancel"
            );
            cancelled.clone()
        }
    };

    let (bytes, ct) = serializer
        .to_vec(&response)
        .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
        .unwrap_or_else(|e| {
            warn!(error = %e, "elicitation response serialization failed, sending Cancel");
            serializer
                .to_vec(&cancelled)
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|_| (Bytes::new(), rpc_reply::CONTENT_TYPE_JSON))
        });
    rpc_reply::publish_reply(nats, reply_to, bytes, ct, "elicitation reply").await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::SessionNotification;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    struct DefaultClient;

    #[async_trait::async_trait(?Send)]
    impl agent_client_protocol::Client for DefaultClient {
        async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
            Ok(())
        }

        async fn request_permission(
            &self,
            _: agent_client_protocol::RequestPermissionRequest,
        ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
            use agent_client_protocol::{RequestPermissionOutcome, RequestPermissionResponse};
            Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
        }
    }

    fn make_request(session_id: &str) -> ElicitationRequest {
        use agent_client_protocol::{ElicitationFormMode, ElicitationMode, ElicitationSchema};
        ElicitationRequest::new(
            session_id.to_string(),
            ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new())),
            "Please provide input",
        )
    }

    fn make_raw_payload(request: &ElicitationRequest) -> Vec<u8> {
        serde_json::to_vec(request).unwrap()
    }

    #[tokio::test]
    async fn default_client_returns_cancel() {
        let client = DefaultClient;
        let request = make_request("sess-1");
        let result = client.request_elicitation(request).await;
        assert!(result.is_ok());
        assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn handle_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, None, &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_success_publishes_cancel_to_reply_subject() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        let parsed: ElicitationResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(matches!(parsed.action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn handle_invalid_payload_publishes_cancel_reply() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;

        handle(b"not json", &client, Some("_INBOX.err"), &nats, "sess-1", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let parsed: ElicitationResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(matches!(parsed.action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn handle_session_id_mismatch_publishes_cancel_reply() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;
        let payload = make_raw_payload(&make_request("sess-other"));

        handle(&payload, &client, Some("_INBOX.err"), &nats, "sess-1", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let parsed: ElicitationResponse = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(matches!(parsed.action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn handle_serialization_fallback_sends_cancel_reply() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;
        let serializer = FailNextSerialize::new(1);
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = DefaultClient;
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn handle_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = DefaultClient;
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, Some("_INBOX.reply"), &nats, "sess-1", &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_double_serialization_failure_sends_empty_bytes() {
        let nats = MockNatsClient::new();
        let client = DefaultClient;
        let serializer = FailNextSerialize::new(2);
        let payload = make_raw_payload(&make_request("sess-1"));

        handle(&payload, &client, Some("_INBOX.err"), &nats, "sess-1", &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }
}
