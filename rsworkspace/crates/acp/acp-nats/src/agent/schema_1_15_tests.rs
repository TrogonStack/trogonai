use super::Bridge;
use super::test_support::{MockJs, mock_bridge, reply_when_published, set_js_response, set_json_response};
use crate::AgentHandler;
use crate::nats::SessionAgentMethod;
use agent_client_protocol::v1::schema::{
    ArchiveSessionRequest, AuthStatusRequest, SessionStatusRequest, SetSessionTitleRequest, UnarchiveSessionRequest,
};
use agent_client_protocol::{Error, ErrorCode, Result};
use jsonrpc_nats::{Message, ResponseId};
use serde_json::{Value, json};
use trogon_nats::AdvancedMockNatsClient;
use trogon_std::time::SystemClock;

fn session_cases() -> [(SessionAgentMethod, Value); 4] {
    [
        (SessionAgentMethod::Archive, json!({})),
        (SessionAgentMethod::Unarchive, json!({})),
        (SessionAgentMethod::Status, json!({"status":"saved"})),
        (SessionAgentMethod::SetTitle, json!({"title":"Renamed"})),
    ]
}

async fn session_call(
    bridge: &Bridge<AdvancedMockNatsClient, SystemClock, MockJs>,
    method: &SessionAgentMethod,
    session_id: &str,
) -> Result<Value> {
    let meta = json!({"probe":"bridge-upgrade"}).as_object().unwrap().clone();
    let result = match method {
        SessionAgentMethod::Archive => serde_json::to_value(
            bridge
                .archive_session(ArchiveSessionRequest::new(session_id.to_owned()).meta(meta))
                .await?,
        ),
        SessionAgentMethod::Unarchive => serde_json::to_value(
            bridge
                .unarchive_session(UnarchiveSessionRequest::new(session_id.to_owned()).meta(meta))
                .await?,
        ),
        SessionAgentMethod::Status => serde_json::to_value(
            bridge
                .session_status(SessionStatusRequest::new(session_id.to_owned()).meta(meta))
                .await?,
        ),
        SessionAgentMethod::SetTitle => serde_json::to_value(
            bridge
                .set_session_title(SetSessionTitleRequest::new(session_id.to_owned(), "Renamed").meta(meta))
                .await?,
        ),
        other => panic!("unexpected schema upgrade method: {other:?}"),
    };
    result.map_err(Error::into_internal_error)
}

#[tokio::test]
async fn new_session_methods_publish_canonical_durable_requests() {
    for (method, expected) in session_cases() {
        let (_nats, js, bridge) = mock_bridge();
        set_js_response(&js, &expected);
        assert_eq!(session_call(&bridge, &method, "s1").await.unwrap(), expected);
        let messages = js.publisher.published_messages();
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(
            message.subject,
            format!("acp.v1.session.s1.agent.{}", method.wire_method())
        );
        let params: Value =
            crate::wire::decode_request_params(method.protocol_method(), &message.headers, &message.payload).unwrap();
        let mut expected_params = json!({"sessionId":"s1","_meta":{"probe":"bridge-upgrade"}});
        if method == SessionAgentMethod::SetTitle {
            expected_params["title"] = json!("Renamed");
        }
        assert_eq!(params, expected_params);
        assert!(crate::wire::req_id_from_request_headers(&message.headers).is_some());
    }
}

#[tokio::test]
async fn new_session_methods_reject_invalid_session_before_publishing() {
    for (method, _) in session_cases() {
        let (nats, js, bridge) = mock_bridge();
        let error = session_call(&bridge, &method, "invalid.session").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(js.publisher.published_messages().is_empty());
        assert!(nats.requested_payloads().is_empty());
    }
}

#[tokio::test]
async fn new_session_methods_preserve_structured_runner_errors() {
    for (method, _) in session_cases() {
        let (_nats, js, bridge) = mock_bridge();
        let (consumer, tx) = trogon_nats::jetstream::MockJetStreamConsumer::new();
        js.consumer_factory.add_consumer(consumer);
        reply_when_published(&js.publisher, tx, |headers| {
            let encoded = crate::wire::encode_agent_error(
                crate::wire::response_id_from_request_headers(&headers),
                &Error::new(-32042, "backend refusal").data(json!({"retryable":false})),
            )
            .unwrap();
            (encoded.headers, encoded.body)
        });
        let error = session_call(&bridge, &method, "s1").await.unwrap_err();
        assert_eq!(i32::from(error.code), -32042);
        assert_eq!(error.message, "backend refusal");
        assert_eq!(error.data, Some(json!({"retryable":false})));
    }
}

#[tokio::test]
async fn auth_status_preserves_global_request_and_response() {
    let (nats, _js, bridge) = mock_bridge();
    let expected =
        json!({"state":"authenticated","accounts":[{"label":"account","active":true}],"_meta":{"result":true}});
    set_json_response(&nats, "acp.v1.global.agent.auth.status", &expected);
    let request = AuthStatusRequest::new().meta(json!({"request":true}).as_object().unwrap().clone());
    let response = bridge.auth_status(request.clone()).await.unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), expected);
    let params: Value = crate::wire::decode_request_params(
        "auth/status",
        &nats.requested_headers()[0],
        &nats.requested_payloads()[0],
    )
    .unwrap();
    assert_eq!(params, serde_json::to_value(request).unwrap());
}

#[tokio::test]
async fn auth_status_preserves_structured_runner_errors() {
    let (nats, _js, bridge) = mock_bridge();
    let encoded = jsonrpc_nats::encode(&Message::Error {
        id: ResponseId::Number(1),
        code: -32042,
        message: "backend refusal".to_owned(),
        data: Some(json!({"retryable":false})),
    })
    .unwrap();
    nats.set_response_wire("acp.v1.global.agent.auth.status", encoded.headers, encoded.body);
    let error = bridge.auth_status(AuthStatusRequest::new()).await.unwrap_err();
    assert_eq!(i32::from(error.code), -32042);
    assert_eq!(error.message, "backend refusal");
    assert_eq!(error.data, Some(json!({"retryable":false})));
}
