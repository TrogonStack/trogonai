use super::test_support::{mock_bridge, set_json_response};
use crate::AgentHandler;
use agent_client_protocol::v1::schema::{SessionId, SessionOptionsRequest};
use jsonrpc_nats::{Message, ResponseId};
use serde_json::{Value, json};

const SESSION_OPTIONS_SUBJECT: &str = "acp.v1.global.agent.session.options";

#[tokio::test]
async fn session_options_preserves_global_request_and_response() {
    let (nats, _js, bridge) = mock_bridge();
    let expected = json!({
        "modes": { "currentModeId": "code", "availableModes": [{ "id": "code", "name": "Code" }] },
        "configOptions": [{
            "type": "select", "id": "speed", "name": "Speed", "category": "speed",
            "currentValue": "fast", "options": [{ "value": "fast", "name": "Fast" }]
        }],
        "_meta": { "result": true }
    });
    set_json_response(&nats, SESSION_OPTIONS_SUBJECT, &expected);
    let request = SessionOptionsRequest::new("/workspace")
        .additional_directories(vec!["/workspace/shared".into()])
        .session_id(SessionId::new("s1"))
        .meta(json!({"request":true}).as_object().unwrap().clone());
    let response = bridge.session_options(request.clone()).await.unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), expected);
    let params: Value = crate::wire::decode_request_params(
        "session/options",
        &nats.requested_headers()[0],
        &nats.requested_payloads()[0],
    )
    .unwrap();
    assert_eq!(params, serde_json::to_value(request).unwrap());
}

#[tokio::test]
async fn session_options_without_session_id_routes_globally() {
    let (nats, _js, bridge) = mock_bridge();
    set_json_response(&nats, SESSION_OPTIONS_SUBJECT, &json!({}));
    bridge
        .session_options(SessionOptionsRequest::new("/workspace"))
        .await
        .unwrap();
    let params: Value = crate::wire::decode_request_params(
        "session/options",
        &nats.requested_headers()[0],
        &nats.requested_payloads()[0],
    )
    .unwrap();
    assert_eq!(params, json!({ "cwd": "/workspace" }));
}

#[tokio::test]
async fn session_options_preserves_structured_runner_errors() {
    let (nats, _js, bridge) = mock_bridge();
    let encoded = jsonrpc_nats::encode(&Message::Error {
        id: ResponseId::Number(1),
        code: -32002,
        message: "Resource not found".to_owned(),
        data: Some(json!({"sessionId":"missing"})),
    })
    .unwrap();
    nats.set_response_wire(SESSION_OPTIONS_SUBJECT, encoded.headers, encoded.body);
    let error = bridge
        .session_options(SessionOptionsRequest::new("/workspace").session_id(SessionId::new("missing")))
        .await
        .unwrap_err();
    assert_eq!(i32::from(error.code), -32002);
    assert_eq!(error.message, "Resource not found");
    assert_eq!(error.data, Some(json!({"sessionId":"missing"})));
}
