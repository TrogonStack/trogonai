use super::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use agent_client_protocol::{PromptResponse, SessionId};
use tracing::{error, instrument, warn};

#[instrument(
    name = "acp.client.ext.session.prompt_response",
    skip(payload, bridge),
    fields(session_id = %session_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(session_id: &str, payload: &[u8], bridge: &Bridge<N>) {
    match serde_json::from_slice::<PromptResponse>(payload) {
        Ok(response) => {
            let session_id_typed: SessionId = session_id.to_string().into();
            if !bridge
                .pending_session_prompt_responses
                .resolve_waiter(&session_id_typed, response)
            {
                warn!(session_id = %session_id, "No pending prompt response waiter found for session");
            }
        }
        Err(e) => {
            error!(error = %e, session_id = %session_id, "Failed to parse prompt response");
        }
    }
}
