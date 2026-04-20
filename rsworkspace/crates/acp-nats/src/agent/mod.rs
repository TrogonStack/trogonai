mod authenticate;
mod bridge;
mod cancel;
mod close_session;
mod ext_method;
mod ext_notification;
mod fork_session;
mod initialize;
pub(crate) mod js_request;
mod list_sessions;
mod load_session;
mod logout;
mod new_session;
mod prompt;
mod resume_session;
mod set_session_config_option;
mod set_session_mode;
mod set_session_model;
#[cfg(test)]
pub(crate) mod test_support;

pub use bridge::Bridge;
pub use prompt::REQ_ID_HEADER;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Bridge;
    use crate::agent::test_support::MockJs;
    use crate::config::Config;
    use agent_client_protocol::{
        Agent, ExtNotification, ExtRequest, PromptRequest, PromptResponse, SessionNotification,
        StopReason,
    };
    use tokio::sync::mpsc;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        MockJs,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock, MockJs>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let js = MockJs::new();
        let (tx, _rx) = mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            js.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tx,
        );
        (mock, js, bridge)
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn drain_background_tasks_completes() {
        let (_mock, _js, bridge) = mock_bridge();
        bridge.spawn_background(tokio::spawn(async {}));
        bridge.drain_background_tasks().await;
        assert!(bridge.background_tasks.borrow().is_empty());
    }

    #[tokio::test]
    async fn prompt_via_agent_trait_returns_done() {
        let (mock, js, bridge) = mock_bridge();

        // cancel sub for core NATS
        let _cancel_tx = mock.inject_messages();

        // notification consumer
        let (notif_consumer, _notif_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(notif_consumer);

        // response consumer
        let (resp_consumer, resp_tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(resp_consumer);

        let response = PromptResponse::new(StopReason::EndTurn);
        let msg = trogon_nats::jetstream::MockJsMessage::new(async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::from(serde_json::to_vec(&response).unwrap()),
            headers: None,
            status: None,
            description: None,
            length: 0,
        });
        resp_tx.unbounded_send(Ok(msg)).unwrap();

        let result = bridge.prompt(PromptRequest::new("s1", vec![])).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn ext_method_returns_agent_unavailable_when_nats_fails() {
        use agent_client_protocol::ErrorCode;

        let (_mock, _js, bridge) = mock_bridge();
        let err = bridge
            .ext_method(ExtRequest::new("ext", empty_raw_value()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn ext_notification_returns_ok_even_when_publish_fails() {
        let (_mock, _js, bridge) = mock_bridge();
        assert!(
            bridge
                .ext_notification(ExtNotification::new("ext", empty_raw_value()))
                .await
                .is_ok()
        );
    }

    /// Methods dispatched via core NATS request_with_timeout return AGENT_UNAVAILABLE
    /// when the request fails — so clients can distinguish agent unavailability from
    /// logic errors.
    #[tokio::test]
    async fn core_nats_methods_return_agent_unavailable_on_failure() {
        use agent_client_protocol::{AuthenticateRequest, ErrorCode, NewSessionRequest};

        let (_mock, _js, bridge) = mock_bridge();

        macro_rules! check_unavailable {
            ($fut:expr) => {{
                let err = $fut.await.unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::Other(crate::error::AGENT_UNAVAILABLE).into(),
                    "core NATS failure must return AGENT_UNAVAILABLE, got {:?}",
                    err.code
                );
            }};
        }

        check_unavailable!(bridge.authenticate(AuthenticateRequest::new("test")));
        check_unavailable!(bridge.new_session(NewSessionRequest::new(".")));
    }

    /// Methods dispatched via JetStream js_request return InternalError when the
    /// JetStream operation fails — JetStream errors are infrastructure-level and
    /// map to InternalError rather than AGENT_UNAVAILABLE.
    #[tokio::test]
    async fn jetstream_methods_return_internal_error_on_failure() {
        use agent_client_protocol::{ErrorCode, LoadSessionRequest, SetSessionModeRequest};

        let (_mock, _js, bridge) = mock_bridge();

        macro_rules! check_internal {
            ($fut:expr) => {{
                let err = $fut.await.unwrap_err();
                assert_eq!(
                    err.code,
                    ErrorCode::InternalError.into(),
                    "JetStream failure must return InternalError, got {:?}",
                    err.code
                );
            }};
        }

        check_internal!(bridge.load_session(LoadSessionRequest::new("s1", ".")));
        check_internal!(bridge.set_session_mode(SetSessionModeRequest::new("s1", "m1")));
    }

    /// cancel is fire-and-forget: it must return Ok(()) even when the underlying
    /// NATS publish fails, so callers are never blocked by backend unavailability.
    #[tokio::test]
    async fn cancel_returns_ok_even_when_nats_publish_fails() {
        use agent_client_protocol::CancelNotification;

        let (mock, _js, bridge) = mock_bridge();
        mock.fail_publish_count(99);

        assert!(
            bridge.cancel(CancelNotification::new("s1")).await.is_ok(),
            "cancel must return Ok(()) regardless of NATS publish failure"
        );
    }
}
