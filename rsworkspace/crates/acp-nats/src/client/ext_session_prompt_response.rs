use super::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use crate::pending_prompt_waiters::PromptToken;
use crate::session_id::AcpSessionId;
use agent_client_protocol::{PromptResponse, SessionId};
use tracing::{error, instrument, warn};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.client.ext.session.prompt_response",
    skip(payload, bridge),
    fields(session_id = %session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient + SubscribeClient,
    C: GetElapsed,
    J,
>(
    session_id: &str,
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
        match serde_json::from_slice::<PromptResponse>(payload) {
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
    let resolved = match bridge.pending_session_prompt_responses.resolve_waiter(
        &session_id_typed,
        prompt_token,
        response_result,
    ) {
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
        bridge.metrics.record_error(
            "client.ext.session.prompt_response",
            "prompt_response_parse_failed",
        );
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
        .and_then(|v| {
            v.get("meta")
                .and_then(|m| m.get("prompt_id"))
                .and_then(|p| p.as_u64())
        })
        .map(PromptToken)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Bridge;
    use crate::config::Config;
    use agent_client_protocol::StopReason;
    use trogon_nats::MockNatsClient;
    use trogon_std::time::MockClock;

    fn make_bridge() -> Bridge<MockNatsClient, MockClock> {
        Bridge::new(
            MockNatsClient::new(),
            MockClock::new(),
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tokio::sync::mpsc::channel(1).0,
        )
    }

    fn response_with_prompt_id(stop_reason: StopReason, prompt_token: PromptToken) -> Vec<u8> {
        let mut meta = serde_json::Map::new();
        meta.insert("prompt_id".to_string(), serde_json::json!(prompt_token.0));
        let response = PromptResponse::new(stop_reason).meta(meta);
        serde_json::to_vec(&response).unwrap()
    }

    #[tokio::test]
    async fn resolves_waiter() {
        let bridge = make_bridge();
        let session_id: SessionId = "prompt-resp-001".into();

        let (rx, token) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();

        let payload = response_with_prompt_id(StopReason::EndTurn, token);

        handle("prompt-resp-001", &payload, None, &bridge).await;

        let result = rx
            .await
            .expect("Should receive response")
            .expect("Prompt response should not include error");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn no_waiter_does_not_panic() {
        let bridge = make_bridge();
        let payload = response_with_prompt_id(StopReason::EndTurn, PromptToken(0));

        handle("no-waiter-session", &payload, None, &bridge).await;
    }

    #[tokio::test]
    async fn invalid_payload_with_prompt_id_forwards_parse_error() {
        let bridge = make_bridge();
        let session_id: SessionId = "bad-payload-001".into();

        let (rx, token) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();

        let payload = format!(
            r#"{{"meta":{{"prompt_id":{}}},"stop_reason":"invalid"}}"#,
            token.0
        );

        handle("bad-payload-001", payload.as_bytes(), None, &bridge).await;

        let result = rx
            .await
            .expect("Should receive resolved parse error")
            .expect_err("Parse failure should be forwarded to waiter");
        assert!(!result.is_empty(), "Expected parse error to be forwarded");
    }

    #[tokio::test]
    async fn missing_prompt_id_is_rejected() {
        let bridge = make_bridge();
        let session_id: SessionId = "no-token-session".into();

        let (rx, _) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();

        let response = PromptResponse::new(StopReason::EndTurn);
        let payload = serde_json::to_vec(&response).unwrap();

        handle("no-token-session", &payload, None, &bridge).await;

        assert!(
            bridge
                .pending_session_prompt_responses
                .has_waiter(&session_id),
            "waiter should remain when response lacks prompt_id"
        );
        bridge
            .pending_session_prompt_responses
            .remove_waiter_for_test(&session_id);
        drop(rx);
    }

    #[tokio::test]
    async fn invalid_session_id_is_rejected() {
        let bridge = make_bridge();
        let session_id: SessionId = "valid-session".into();

        let (rx, token) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();

        let payload = response_with_prompt_id(StopReason::EndTurn, token);

        handle("session.with.dots", &payload, None, &bridge).await;
        handle("session*wild", &payload, None, &bridge).await;
        handle("session id", &payload, None, &bridge).await;

        assert!(
            bridge
                .pending_session_prompt_responses
                .has_waiter(&session_id),
            "invalid session IDs should not resolve valid waiter",
        );

        bridge
            .pending_session_prompt_responses
            .remove_waiter_for_test(&session_id);
        assert!(
            !bridge
                .pending_session_prompt_responses
                .has_waiter(&session_id),
            "waiter should be removed"
        );
        drop(rx);
    }

    #[tokio::test]
    async fn late_response_with_wrong_token_does_not_resolve_new_prompt() {
        let bridge = make_bridge();
        let session_id: SessionId = "same-session".into();

        let (_rx1, token1) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();
        bridge
            .pending_session_prompt_responses
            .resolve_waiter(
                &session_id,
                token1,
                Ok(PromptResponse::new(StopReason::EndTurn)),
            )
            .unwrap();
        let _ = _rx1.await;

        let (rx2, token2) = bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .unwrap();

        let late_payload = response_with_prompt_id(StopReason::EndTurn, token1);
        handle("same-session", &late_payload, None, &bridge).await;

        assert!(
            bridge
                .pending_session_prompt_responses
                .has_waiter(&session_id),
            "late response with old token must not resolve new prompt"
        );
        bridge
            .pending_session_prompt_responses
            .resolve_waiter(
                &session_id,
                token2,
                Ok(PromptResponse::new(StopReason::EndTurn)),
            )
            .unwrap();
        let result = rx2.await.unwrap().unwrap();
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }
}
