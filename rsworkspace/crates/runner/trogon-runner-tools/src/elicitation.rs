//! Shared elicitation bridge: Runner → LocalSet handler → ACP client.
//!
//! When a runner's agent loop needs free-text input from the user (the
//! `ask_user` tool), it builds an [`ElicitationReq`] and sends it over an mpsc
//! channel to a LocalSet task. That task forwards the request to the ACP client
//! via [`handle_elicitation_request_nats`] (NATS request-reply) and the answer
//! comes back on the embedded oneshot channel.
//!
//! Runners that own their tool loop (xai, openrouter) call
//! [`elicit_via_channel`] directly. Runners built on `trogon-agent-core` wrap a
//! provider around the same primitives.

use std::time::Duration;

use acp_nats::{acp_prefix::AcpPrefix, client_proxy::NatsClientProxy, session_id::AcpSessionId, ClientHandler};
use agent_client_protocol::schema::v1::{
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationScope, ElicitationSessionScope,
};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;
use trogon_nats::{FlushClient, PublishClient, RequestClient};

/// A single elicitation request sent from a runner to the ACP connection handler.
pub struct ElicitationReq {
    pub request: CreateElicitationRequest,
    /// Send the client's response (or an error) back to the agent.
    pub response_tx: oneshot::Sender<agent_client_protocol::Result<CreateElicitationResponse>>,
}

/// Sender half — given to a runner so it can forward elicitation requests.
pub type ElicitationTx = mpsc::Sender<ElicitationReq>;

/// Extract the user's free-text answer from an elicitation response, encoding
/// non-string values as their string representation. Returns `None` if the user
/// cancelled or no `"answer"` field is present.
pub fn answer_from_response(response: CreateElicitationResponse) -> Option<String> {
    match response.action {
        ElicitationAction::Accept(ElicitationAcceptAction {
            content: Some(fields), ..
        }) => fields.get("answer").map(|v| match v {
            ElicitationContentValue::String(s) => s.clone(),
            ElicitationContentValue::Integer(n) => n.to_string(),
            ElicitationContentValue::Number(n) => n.to_string(),
            ElicitationContentValue::Boolean(b) => b.to_string(),
            ElicitationContentValue::StringArray(arr) => arr.join(", "),
            _ => String::new(),
        }),
        _ => None,
    }
}

/// Ask the user `question` for `session_id` by round-tripping an elicitation
/// request through `tx`. Returns the answer, or `None` if the user cancelled or
/// the bridge failed (channel closed, dropped response, NATS error).
pub async fn elicit_via_channel(tx: &ElicitationTx, session_id: &str, question: &str) -> Option<String> {
    let (response_tx, response_rx) = oneshot::channel();
    let schema = ElicitationSchema::new().string("answer", true);
    let scope = ElicitationSessionScope::new(session_id.to_string());
    let request = CreateElicitationRequest::new(ElicitationFormMode::new(scope, schema), question);
    if tx.send(ElicitationReq { request, response_tx }).await.is_err() {
        return None;
    }
    match response_rx.await {
        Ok(Ok(response)) => answer_from_response(response),
        _ => None,
    }
}

/// Forward a single [`ElicitationReq`] to the ACP client and send the response
/// back on the embedded oneshot channel.
///
/// On network error or timeout the error is forwarded to the caller via the
/// oneshot; the caller treats a failure as a cancelled elicitation.
pub async fn handle_elicitation_request_nats<N>(req: ElicitationReq, nats: N, prefix: AcpPrefix)
where
    N: RequestClient + PublishClient + FlushClient,
{
    let session_id_str = match req.request.scope() {
        ElicitationScope::Session(scope) => scope.session_id.to_string(),
        _ => {
            warn!("elicitation request is request-scoped; cannot route via a session-bound NatsClientProxy");
            return;
        }
    };

    let session_id = match AcpSessionId::new(&session_id_str) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id_str,
                "invalid session_id in elicitation request"
            );
            // We can't recover; dropping response_tx signals failure to the caller.
            return;
        }
    };

    let proxy = NatsClientProxy::new(nats, session_id, prefix, Duration::from_secs(30));
    let result = proxy.elicitation_create(req.request).await;
    let _ = req.response_tx.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use trogon_nats::AdvancedMockNatsClient;

    fn accept_string(answer: &str) -> CreateElicitationResponse {
        let mut content = BTreeMap::new();
        content.insert(
            "answer".to_string(),
            ElicitationContentValue::String(answer.to_string()),
        );
        CreateElicitationResponse::new(ElicitationAction::Accept(
            ElicitationAcceptAction::new().content(content),
        ))
    }

    /// `elicit_via_channel` returns the `"answer"` field from an Accept response.
    #[tokio::test]
    async fn elicit_returns_answer() {
        let (tx, mut rx) = mpsc::channel::<ElicitationReq>(8);
        let task = tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(Ok(accept_string("hello")));
            }
        });
        let result = elicit_via_channel(&tx, "sess-1", "What is your name?").await;
        task.await.unwrap();
        assert_eq!(result, Some("hello".to_string()));
    }

    /// `elicit_via_channel` returns `None` when the user cancels.
    #[tokio::test]
    async fn elicit_cancel_returns_none() {
        let (tx, mut rx) = mpsc::channel::<ElicitationReq>(8);
        let task = tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req
                    .response_tx
                    .send(Ok(CreateElicitationResponse::new(ElicitationAction::Cancel)));
            }
        });
        let result = elicit_via_channel(&tx, "sess-1", "Continue?").await;
        task.await.unwrap();
        assert_eq!(result, None);
    }

    /// `elicit_via_channel` returns `None` when the channel is closed.
    #[tokio::test]
    async fn elicit_closed_channel_returns_none() {
        let (tx, rx) = mpsc::channel::<ElicitationReq>(1);
        drop(rx);
        assert_eq!(elicit_via_channel(&tx, "s", "test?").await, None);
    }

    /// Non-string answers are encoded as their string representation.
    #[tokio::test]
    async fn elicit_integer_answer_stringified() {
        let (tx, mut rx) = mpsc::channel::<ElicitationReq>(8);
        let task = tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let mut content = BTreeMap::new();
                content.insert("answer".to_string(), ElicitationContentValue::Integer(42));
                let resp = CreateElicitationResponse::new(ElicitationAction::Accept(
                    ElicitationAcceptAction::new().content(content),
                ));
                let _ = req.response_tx.send(Ok(resp));
            }
        });
        let result = elicit_via_channel(&tx, "sess-1", "Pick a number").await;
        task.await.unwrap();
        assert_eq!(result, Some("42".to_string()));
    }

    const SUBJECT: &str = "acp.session.sess-1.client.elicitation.create";

    fn make_req(
        session_id: &str,
    ) -> (
        ElicitationReq,
        oneshot::Receiver<agent_client_protocol::Result<CreateElicitationResponse>>,
    ) {
        let (tx, rx) = oneshot::channel();
        let scope = ElicitationSessionScope::new(session_id.to_string());
        let request = CreateElicitationRequest::new(
            ElicitationFormMode::new(scope, ElicitationSchema::new()),
            "Please enter a value",
        );
        (
            ElicitationReq {
                request,
                response_tx: tx,
            },
            rx,
        )
    }

    #[tokio::test]
    async fn cancel_response_forwarded_to_caller() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(
            SUBJECT,
            serde_json::to_vec(&CreateElicitationResponse::new(ElicitationAction::Cancel))
                .unwrap()
                .into(),
        );

        let (req, rx) = make_req("sess-1");
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;

        let result = rx.await.unwrap();
        assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn nats_error_sends_error_on_oneshot() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

        let (req, rx) = make_req("sess-1");
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;

        assert!(rx.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn invalid_session_id_drops_response_tx() {
        let nats = AdvancedMockNatsClient::new();
        let (req, rx) = make_req("invalid.session.id");
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;
        assert!(rx.await.is_err());
    }
}
