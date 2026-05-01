//! In-process elicitation bridge: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop needs structured input from the user, it sends an
//! `ElicitationReq` over an mpsc channel to the LocalSet task, which forwards
//! it to the ACP client via `NatsClientProxy::request_elicitation` (NATS
//! request-reply) and sends the response back on the embedded oneshot channel.

use std::time::Duration;

use agent_client_protocol::{ElicitationRequest, ElicitationResponse};
use acp_nats::{acp_prefix::AcpPrefix, client_proxy::NatsClientProxy, session_id::AcpSessionId};
use tokio::sync::{mpsc, oneshot};
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
