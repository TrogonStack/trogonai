use agent_client_protocol::{Error, ErrorCode};
use bytes::Bytes;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::time::timeout;
use trogon_nats::REQ_ID_HEADER;
use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamConsumerFactory, JetStreamPublisher, JsMessage as _,
};
use trogon_std::JsonSerialize;

use crate::constants::SESSION_ID_HEADER;
use crate::jetstream::{consumers, streams};

#[allow(clippy::too_many_arguments)]
pub async fn js_request<J, Req, Res, S>(
    js: &J,
    subject: &str,
    request: &Req,
    serializer: &S,
    prefix: &str,
    session_id: &str,
    req_id: &str,
    operation_timeout: Duration,
) -> agent_client_protocol::Result<Res>
where
    J: JetStreamPublisher + JetStreamConsumerFactory,
    Req: serde::Serialize,
    Res: DeserializeOwned,
    S: JsonSerialize,
{
    // Create consumer BEFORE publishing — prevents missing the response if the
    // runner responds before we start consuming. DeliverAll replays from stream start.
    let responses_stream = streams::responses_stream_name(prefix);
    let resp_config = consumers::response_consumer(prefix, session_id, req_id);
    let resp_consumer: J::Consumer = js
        .create_consumer(&responses_stream, resp_config)
        .await
        .map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("create response consumer: {e}"),
            )
        })?;
    let mut resp_messages: <J::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages =
        resp_consumer.messages().await.map_err(|e| {
            Error::new(
                ErrorCode::InternalError.into(),
                format!("response messages: {e}"),
            )
        })?;

    let payload_bytes = serializer
        .to_vec(request)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("serialize: {e}")))?;

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, req_id);
    headers.insert(SESSION_ID_HEADER, session_id);

    js.js_publish_with_headers(subject.to_string(), headers, Bytes::from(payload_bytes))
        .await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), format!("js publish: {e}")))?;

    match timeout(operation_timeout, resp_messages.next()).await {
        Ok(Some(Ok(js_msg))) => match serde_json::from_slice::<Res>(js_msg.payload()) {
            Ok(response) => {
                let _ = js_msg.ack().await;
                Ok(response)
            }
            Err(_) => {
                if let Ok(agent_err) = serde_json::from_slice::<Error>(js_msg.payload()) {
                    let _ = js_msg.ack().await;
                    Err(agent_err)
                } else {
                    let _ = js_msg.term().await;
                    Err(Error::new(
                        ErrorCode::InternalError.into(),
                        "bad response payload",
                    ))
                }
            }
        },
        Ok(Some(Err(e))) => Err(Error::new(
            ErrorCode::InternalError.into(),
            format!("response consumer: {e}"),
        )),
        Ok(None) => Err(Error::new(
            ErrorCode::InternalError.into(),
            "response stream closed unexpectedly",
        )),
        Err(_elapsed) => Err(Error::new(
            ErrorCode::InternalError.into(),
            "request timed out waiting for runner",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::PromptResponse;
    use trogon_nats::jetstream::mocks::*;

    #[derive(Clone)]
    struct MockJs {
        publisher: MockJetStreamPublisher,
        consumer_factory: MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: MockJetStreamPublisher::new(),
                consumer_factory: MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl JetStreamPublisher for MockJs {
        type PublishError = trogon_nats::mocks::MockError;

        async fn js_publish_with_headers(
            &self,
            subject: String,
            headers: async_nats::HeaderMap,
            payload: Bytes,
        ) -> Result<async_nats::jetstream::publish::PublishAck, Self::PublishError> {
            self.publisher
                .js_publish_with_headers(subject, headers, payload)
                .await
        }
    }

    impl JetStreamConsumerFactory for MockJs {
        type Error = trogon_nats::mocks::MockError;
        type Consumer = MockJetStreamConsumer;

        async fn create_consumer(
            &self,
            stream_name: &str,
            config: async_nats::jetstream::consumer::pull::Config,
        ) -> Result<MockJetStreamConsumer, Self::Error> {
            self.consumer_factory
                .create_consumer(stream_name, config)
                .await
        }
    }

    fn make_nats_msg(payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    #[tokio::test]
    async fn js_request_success() {
        let js = MockJs::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let response = PromptResponse::new(agent_client_protocol::StopReason::EndTurn);
        let msg = MockJsMessage::new(make_nats_msg(&serde_json::to_vec(&response).unwrap()));
        tx.unbounded_send(msg).unwrap();

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().stop_reason,
            agent_client_protocol::StopReason::EndTurn
        );
    }

    #[tokio::test]
    async fn js_request_publish_failure() {
        let js = MockJs::new();
        let (consumer, _tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);
        js.publisher.fail_next_js_publish();

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("js publish"));
    }

    #[tokio::test]
    async fn js_request_consumer_creation_failure() {
        let js = MockJs::new();

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("create response consumer")
        );
    }

    #[tokio::test]
    async fn js_request_messages_failure() {
        let js = MockJs::new();
        let failing_consumer = MockJetStreamConsumer::failing();
        js.consumer_factory.add_consumer(failing_consumer);

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("response messages"));
    }

    #[tokio::test]
    async fn js_request_bad_response_payload() {
        let js = MockJs::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg(b"not json"));
        tx.unbounded_send(msg).unwrap();

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("bad response payload"));
    }

    #[tokio::test]
    async fn js_request_timeout() {
        let js = MockJs::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }

    #[tokio::test]
    async fn js_request_agent_error_response() {
        let js = MockJs::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let agent_err = agent_client_protocol::Error::new(
            agent_client_protocol::ErrorCode::InternalError.into(),
            "agent failed",
        );
        let msg = MockJsMessage::new(async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: Bytes::from(serde_json::to_vec(&agent_err).unwrap()),
            headers: None,
            status: None,
            description: None,
            length: 0,
        });
        tx.unbounded_send(msg).unwrap();

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("agent failed"));
    }

    #[tokio::test]
    async fn js_request_stream_closed() {
        let js = MockJs::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        drop(tx);

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::StdJsonSerialize,
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message
                .contains("response stream closed")
        );
    }

    #[tokio::test]
    async fn js_request_serialize_failure() {
        let js = MockJs::new();
        let (consumer, _tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);

        let result: agent_client_protocol::Result<PromptResponse> = js_request(
            &js,
            "acp.session.s1.agent.prompt",
            &agent_client_protocol::PromptRequest::new("s1", vec![]),
            &trogon_std::FailNextSerialize::new(1),
            "acp",
            "s1",
            "req-1",
            Duration::from_secs(5),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("serialize"));
    }
}
