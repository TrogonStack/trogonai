use super::Bridge;
use crate::session_id::AcpSessionId;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use agent_client_protocol::{PromptResponse, SessionId};
use tracing::{instrument, warn};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.client.ext.session.prompt_response",
    skip(payload, bridge),
    fields(session_id = %session_id)
)]
pub async fn handle<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
>(
    session_id: &str,
    payload: &[u8],
    bridge: &Bridge<N, C>,
) {
    let Ok(validated) = AcpSessionId::new(session_id) else {
        warn!(
            session_id = %session_id,
            "Invalid session_id in prompt response notification"
        );
        bridge
            .metrics
            .record_error("client.ext.session.prompt_response", "invalid_session_id");
        return;
    };

    let session_id_typed: SessionId = validated.as_str().to_string().into();

    match serde_json::from_slice::<PromptResponse>(payload) {
        Ok(response) => {
            bridge
                .pending_session_prompt_responses
                .purge_expired_timed_out_waiters(&bridge.clock);
            let suppress_missing_waiter_warning = bridge
                .pending_session_prompt_responses
                .should_suppress_missing_waiter_warning(&session_id_typed, &bridge.clock);

            if !bridge
                .pending_session_prompt_responses
                .resolve_waiter(&session_id_typed, Ok(response))
            {
                if !suppress_missing_waiter_warning {
                    warn!(
                        session_id = %session_id,
                        "No pending prompt response waiter found for session"
                    );
                }
            }
        }
        Err(e) => {
            warn!(error = %e, session_id = %session_id, "Failed to parse prompt response");
            bridge
                .pending_session_prompt_responses
                .purge_expired_timed_out_waiters(&bridge.clock);
            let suppress_missing_waiter_warning = bridge
                .pending_session_prompt_responses
                .should_suppress_missing_waiter_warning(&session_id_typed, &bridge.clock);

            if !bridge
                .pending_session_prompt_responses
                .resolve_waiter(&session_id_typed, Err(e.to_string()))
            {
                if !suppress_missing_waiter_warning {
                    warn!(
                        session_id = %session_id,
                        "No pending prompt response waiter found for session"
                    );
                }
            }
            bridge
                .metrics
                .record_error("client.ext.session.prompt_response", "prompt_response_parse_failed");
        }
    }
}
