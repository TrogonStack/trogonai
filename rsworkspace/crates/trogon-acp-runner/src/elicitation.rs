//! In-process elicitation bridge: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop needs structured input from the user, it sends an
//! `ElicitationReq` over an mpsc channel to the LocalSet task, which forwards
//! it to the ACP client via `NatsClientProxy::request_elicitation` (NATS
//! request-reply) and sends the response back on the embedded oneshot channel.

use std::time::Duration;

use agent_client_protocol::{
    ElicitationAcceptAction, ElicitationAction, ElicitationContentValue, ElicitationFormMode,
    ElicitationMode, ElicitationRequest, ElicitationResponse, ElicitationSchema,
};
use acp_nats::{acp_prefix::AcpPrefix, client_proxy::NatsClientProxy, session_id::AcpSessionId};
use tokio::sync::{mpsc, oneshot};
use trogon_agent_core::agent_loop::ElicitationProvider;
use trogon_nats::{FlushClient, PublishClient, RequestClient};
use tracing::warn;

/// A single elicitation request sent from the Runner to the ACP connection handler.
pub struct ElicitationReq {
    pub request: ElicitationRequest,
    /// Send the client's response (or an error) back to the agent.
    pub response_tx: oneshot::Sender<agent_client_protocol::Result<ElicitationResponse>>,
}

/// Sender half — given to the Runner so it can forward elicitation requests.
pub type ElicitationTx = mpsc::Sender<ElicitationReq>;

/// Implements `ElicitationProvider` by routing requests through the channel to
/// the LocalSet task, which forwards them to the ACP client via NATS.
pub struct ChannelElicitationProvider {
    pub session_id: String,
    pub tx: ElicitationTx,
}

impl ElicitationProvider for ChannelElicitationProvider {
    fn elicit<'a>(
        &'a self,
        question: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            let (response_tx, response_rx) = oneshot::channel();
            let schema = ElicitationSchema::new().string("answer", true);
            let request = ElicitationRequest::new(
                self.session_id.clone(),
                ElicitationMode::Form(ElicitationFormMode::new(schema)),
                question,
            );
            let req = ElicitationReq { request, response_tx };
            if self.tx.send(req).await.is_err() {
                return None;
            }
            match response_rx.await {
                Ok(Ok(response)) => match response.action {
                    ElicitationAction::Accept(ElicitationAcceptAction { content: Some(fields), .. }) => {
                        fields.get("answer").map(|v| match v {
                            ElicitationContentValue::String(s) => s.clone(),
                            ElicitationContentValue::Integer(n) => n.to_string(),
                            ElicitationContentValue::Number(n) => n.to_string(),
                            ElicitationContentValue::Boolean(b) => b.to_string(),
                            ElicitationContentValue::StringArray(arr) => arr.join(", "),
                            _ => String::new(),
                        })
                    }
                    _ => None,
                },
                _ => None,
            }
        })
    }
}

/// Forward a single `ElicitationReq` to the ACP client and send the response
/// back on the embedded oneshot channel.
///
/// On network error or timeout the error is forwarded to the caller via the
/// oneshot channel; the caller is responsible for deciding the fallback action
/// (typically treating the elicitation as cancelled).
pub async fn handle_elicitation_request_nats<N>(req: ElicitationReq, nats: N, prefix: AcpPrefix)
where
    N: RequestClient + PublishClient + FlushClient,
{
    let session_id_str = req.request.session_id.to_string();

    let session_id = match AcpSessionId::new(&session_id_str) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id_str,
                "invalid session_id in elicitation request"
            );
            // We can't recover; drop the response_tx which signals failure to the caller.
            return;
        }
    };

    let proxy = NatsClientProxy::new(nats, session_id, prefix, Duration::from_secs(30));
    let result = proxy.request_elicitation(req.request).await;
    let _ = req.response_tx.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        ElicitationAction, ElicitationFormMode, ElicitationMode, ElicitationResponse, ElicitationSchema,
    };
    use trogon_nats::AdvancedMockNatsClient;

    const SESSION: &str = "sess-1";
    const SUBJECT: &str = "acp.session.sess-1.client.session.elicitation";

    fn make_req(session_id: &str) -> (ElicitationReq, oneshot::Receiver<agent_client_protocol::Result<ElicitationResponse>>) {
        let (tx, rx) = oneshot::channel();
        let request = ElicitationRequest::new(
            session_id.to_string(),
            ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new())),
            "Please enter a value",
        );
        let req = ElicitationReq {
            request,
            response_tx: tx,
        };
        (req, rx)
    }

    fn cancel_response() -> bytes::Bytes {
        serde_json::to_vec(&ElicitationResponse::new(ElicitationAction::Cancel))
            .unwrap()
            .into()
    }

    #[tokio::test]
    async fn cancel_response_forwarded_to_caller() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response(SUBJECT, cancel_response());

        let (req, rx) = make_req(SESSION);
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;

        let result = rx.await.unwrap();
        assert!(result.is_ok());
        assert!(matches!(result.unwrap().action, ElicitationAction::Cancel));
    }

    #[tokio::test]
    async fn nats_error_sends_error_on_oneshot() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_request();

        let (req, rx) = make_req(SESSION);
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;

        let result = rx.await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_session_id_drops_response_tx() {
        let nats = AdvancedMockNatsClient::new();

        let (req, rx) = make_req("invalid.session.id");
        handle_elicitation_request_nats(req, nats, AcpPrefix::new("acp").unwrap()).await;

        // response_tx was dropped without sending → rx returns Err
        assert!(rx.await.is_err());
    }
}
