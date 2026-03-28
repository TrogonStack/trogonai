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
    use crate::config::Config;
    use agent_client_protocol::{
        Agent, ExtNotification, ExtRequest, PromptRequest, PromptResponse, SessionNotification,
        StopReason,
    };
    use tokio::sync::mpsc;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let (tx, _rx) = mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tx,
        );
        (mock, bridge)
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn drain_background_tasks_completes() {
        let (_mock, bridge) = mock_bridge();
        bridge.spawn_background(tokio::spawn(async {}));
        bridge.drain_background_tasks().await;
        assert!(bridge.background_tasks.borrow().is_empty());
    }

    #[tokio::test]
    async fn prompt_via_agent_trait_returns_done() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(async_nats::Message {
                subject: "test".into(),
                reply: None,
                payload: bytes::Bytes::from(serde_json::to_vec(&response).unwrap()),
                headers: None,
                status: None,
                description: None,
                length: 0,
            })
            .unwrap();

        let result = bridge.prompt(PromptRequest::new("s1", vec![])).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn ext_method_returns_agent_unavailable_when_nats_fails() {
        use agent_client_protocol::ErrorCode;

        let (_mock, bridge) = mock_bridge();
        let err = bridge
            .ext_method(ExtRequest::new("ext", empty_raw_value()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn ext_notification_returns_ok_even_when_publish_fails() {
        let (_mock, bridge) = mock_bridge();
        assert!(
            bridge
                .ext_notification(ExtNotification::new("ext", empty_raw_value()))
                .await
                .is_ok()
        );
    }
}
