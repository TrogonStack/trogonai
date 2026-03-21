use agent_client_protocol::{
    Error, ErrorCode, PromptRequest, PromptResponse, SessionNotification, StopReason,
};
use bytes::Bytes;
use futures::StreamExt;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

use crate::agent::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use crate::session_id::AcpSessionId;

pub const REQ_ID_HEADER: &str = "X-Req-Id";

#[instrument(
    name = "acp.session.prompt",
    skip(bridge, args, serializer),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N, C, S>(
    bridge: &Bridge<N, C>,
    args: PromptRequest,
    serializer: &S,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: trogon_std::time::GetElapsed,
    S: JsonSerialize,
{
    let start = bridge.clock.now();

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        bridge.metrics.record_error("prompt", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), "Invalid session ID")
    })?;

    let req_id = uuid::Uuid::new_v4().to_string();
    let sid = session_id.as_ref();
    let prefix = bridge.config.acp_prefix();

    // Subscribe BEFORE publishing — prevents losing the first event if the runner responds instantly.
    let mut notifications_sub = bridge
        .nats
        .subscribe(agent::session_update(prefix, sid, &req_id))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}")))?;

    let mut response_sub = bridge
        .nats
        .subscribe(agent::ext_session_prompt_response(prefix, sid, &req_id))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("subscribe: {e}")))?;

    let mut cancel_sub = bridge
        .nats
        .subscribe(agent::session_cancelled(prefix, sid))
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("subscribe cancelled: {e}"),
            )
        })?;

    let payload_bytes = serializer
        .to_vec(&args)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("serialize: {e}")))?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id.as_str());

    let prompt_subject = agent::session_prompt(prefix, sid);
    bridge
        .nats
        .publish_with_headers(prompt_subject, headers, Bytes::from(payload_bytes))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("publish: {e}")))?;

    bridge
        .nats
        .flush()
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("flush: {e}")))?;

    let op_timeout = bridge.config.prompt_timeout();

    let result = loop {
        tokio::select! {
            notif = notifications_sub.next() => {
                let Some(msg) = notif else {
                    bridge.metrics.record_error("prompt", "notification_stream_closed");
                    break Err(Error::new(
                        ErrorCode::InternalError.into(),
                        "notification stream closed unexpectedly",
                    ));
                };
                let notification: SessionNotification = match serde_json::from_slice(&msg.payload) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(error = %e, "bad notification payload; skipping");
                        continue;
                    }
                };
                if bridge.notification_sender.send(notification).await.is_err() {
                    warn!("notification receiver dropped; continuing prompt");
                }
            }
            resp = timeout(op_timeout, response_sub.next()) => {
                match resp {
                    Ok(Some(msg)) => {
                        match serde_json::from_slice::<PromptResponse>(&msg.payload) {
                            Ok(response) => break Ok(response),
                            Err(e) => {
                                bridge.metrics.record_error("prompt", "bad_response_payload");
                                break Err(Error::new(
                                    ErrorCode::InternalError.into(),
                                    format!("bad response payload: {e}"),
                                ));
                            }
                        }
                    }
                    Ok(None) => {
                        bridge.metrics.record_error("prompt", "response_stream_closed");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "response stream closed unexpectedly",
                        ));
                    }
                    Err(_elapsed) => {
                        bridge.metrics.record_error("prompt", "prompt_timeout");
                        break Err(Error::new(
                            ErrorCode::InternalError.into(),
                            "prompt timed out waiting for runner",
                        ));
                    }
                }
            }
            _ = cancel_sub.next() => {
                break Ok(PromptResponse::new(StopReason::Cancelled));
            }
        }
    };

    bridge.metrics.record_request(
        "prompt",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use trogon_nats::AdvancedMockNatsClient;

    fn make_nats_msg(payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let (notification_tx, _notification_rx) =
            tokio::sync::mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("prompt-test"),
            Config::for_test("acp"),
            notification_tx,
        );
        (mock, bridge)
    }

    #[tokio::test]
    async fn prompt_returns_error_when_subscribe_fails() {
        let (_mock, bridge) = mock_bridge();
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_rejects_invalid_session_id() {
        let (_mock, bridge) = mock_bridge();
        let err = handle(
            &bridge,
            PromptRequest::new("invalid.session.id", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn prompt_returns_done_response_from_runner() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(make_nats_msg(&serde_json::to_vec(&response).unwrap()))
            .unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn prompt_returns_cancelled_on_cancel_signal() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let cancel_tx = mock.inject_messages();

        cancel_tx.unbounded_send(make_nats_msg(b"")).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::Cancelled);
    }

    #[tokio::test]
    async fn prompt_returns_error_on_bad_response_payload() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        resp_tx.unbounded_send(make_nats_msg(b"not json")).unwrap();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_response_stream_closes() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        drop(resp_tx);

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_second_subscribe_fails() {
        let (mock, bridge) = mock_bridge();
        let _notif_tx = mock.inject_messages();
        // No second stream — second subscribe fails
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_third_subscribe_fails() {
        let (mock, bridge) = mock_bridge();
        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        // No third stream — third subscribe fails
        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_serialize_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::FailNextSerialize::new(1),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_publish_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();
        mock.fail_next_publish();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_returns_error_when_flush_fails() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let _resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();
        mock.fail_next_flush();

        let result = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn prompt_publishes_to_correct_subject() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(make_nats_msg(&serde_json::to_vec(&response).unwrap()))
            .unwrap();

        let _ = handle(
            &bridge,
            PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
        )
        .await;

        let subjects = mock.published_messages();
        assert!(
            subjects.iter().any(|s| s == "acp.s1.agent.session.prompt"),
            "expected publish to acp.s1.agent.session.prompt, got: {:?}",
            subjects
        );
    }
}
