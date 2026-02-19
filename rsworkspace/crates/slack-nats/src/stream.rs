use slack_types::{
    events::{SlackStreamStartRequest, SlackStreamStartResponse},
    subjects::SLACK_OUTBOUND_STREAM_START,
};
use trogon_nats::{NatsError, RequestClient, request};

pub async fn request_stream_start<N: RequestClient>(
    client: &N,
    req: &SlackStreamStartRequest,
) -> Result<SlackStreamStartResponse, NatsError> {
    request(client, SLACK_OUTBOUND_STREAM_START, req).await
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn request_stream_start_sends_to_correct_subject_and_deserializes_response() {
        let mock = AdvancedMockNatsClient::new();
        let response = SlackStreamStartResponse {
            channel: "C1".to_string(),
            ts: "9876543210.000".to_string(),
        };
        let response_bytes =
            bytes::Bytes::from(serde_json::to_vec(&response).expect("serialize response"));
        mock.set_response(SLACK_OUTBOUND_STREAM_START, response_bytes);

        let req = SlackStreamStartRequest {
            channel: "C1".to_string(),
            thread_ts: None,
            initial_text: Some("Thinking…".to_string()),
        };
        let result = request_stream_start(&mock, &req).await;
        assert!(result.is_ok(), "expected Ok but got: {:?}", result);
        let resp = result.unwrap();
        assert_eq!(resp.channel, "C1");
        assert_eq!(resp.ts, "9876543210.000");
    }

    #[tokio::test]
    async fn request_stream_start_propagates_error_when_no_response_configured() {
        let mock = AdvancedMockNatsClient::new();
        let req = SlackStreamStartRequest {
            channel: "C1".to_string(),
            thread_ts: None,
            initial_text: None,
        };
        let result = request_stream_start(&mock, &req).await;
        assert!(result.is_err());
    }
}
