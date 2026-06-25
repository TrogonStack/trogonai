use super::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use crate::pending_prompt_waiters::PromptToken;
use crate::session_id::AcpSessionId;
use crate::wire::decode_notification_params;
use agent_client_protocol::{PromptResponse, SessionId};
use async_nats::header::HeaderMap;
use tracing::{error, instrument, warn};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.client.ext.session.prompt_response",
    skip(headers, payload, bridge),
    fields(session_id = %session_id)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient + SubscribeClient, C: GetElapsed, J>(
    session_id: &str,
    headers: &HeaderMap,
    payload: &[u8],
    reply: Option<&str>,
    bridge: &Bridge<N, C, J>,
) {
    if reply.is_some() {
        warn!(
            session_id = %session_id,
            "Unexpected reply subject on prompt response notification"
        );
    }

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

    let (prompt_token_opt, response_result) =
        match decode_notification_params::<PromptResponse>("ext/session/prompt_response", headers, payload) {
            Ok(response) => (extract_prompt_token(&response), Ok(response)),
            Err(e) => {
                let token = extract_prompt_token_from_raw(payload);
                (token, Err(e.to_string()))
            }
        };

    let Some(prompt_token) = prompt_token_opt else {
        warn!(
            session_id = %session_id,
            "Prompt response missing prompt_id in meta; cannot correlate"
        );
        bridge
            .metrics
            .record_error("client.ext.session.prompt_response", "missing_prompt_id");
        return;
    };

    // error! is justified here: mutex poisoning means a thread panicked while
    // holding the lock — process state is corrupted and unrecoverable.
    if let Err(e) = bridge
        .pending_session_prompt_responses
        .purge_expired_timed_out_waiters(&bridge.clock)
    {
        error!(error = %e, "Lock poisoned in purge_expired_timed_out_waiters");
        return;
    }
    let suppress_missing_waiter_warning = match bridge
        .pending_session_prompt_responses
        .should_suppress_missing_waiter_warning(&session_id_typed, prompt_token, &bridge.clock)
    {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "Lock poisoned in should_suppress_missing_waiter_warning");
            return;
        }
    };

    let parse_failed = response_result.is_err();
    let resolved =
        match bridge
            .pending_session_prompt_responses
            .resolve_waiter(&session_id_typed, prompt_token, response_result)
        {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "Lock poisoned in resolve_waiter");
                return;
            }
        };
    if !resolved && !suppress_missing_waiter_warning {
        warn!(
            session_id = %session_id,
            "No pending prompt response waiter found for session"
        );
    }

    if parse_failed {
        bridge
            .metrics
            .record_error("client.ext.session.prompt_response", "prompt_response_parse_failed");
    }
}

fn extract_prompt_token(response: &PromptResponse) -> Option<PromptToken> {
    response
        .meta
        .as_ref()
        .and_then(|m| m.get("prompt_id"))
        .and_then(|v| v.as_u64())
        .map(PromptToken)
}

fn extract_prompt_token_from_raw(payload: &[u8]) -> Option<PromptToken> {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("meta").and_then(|m| m.get("prompt_id")).and_then(|p| p.as_u64()))
        .map(PromptToken)
}

#[cfg(test)]
mod tests;
