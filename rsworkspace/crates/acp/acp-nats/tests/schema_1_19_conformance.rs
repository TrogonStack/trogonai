#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use acp_nats::wire::{
    decode_notification_params, decode_request_params, decode_response, encode_notification, encode_request,
    encode_success,
};
use agent_client_protocol::v1::schema::{
    AuthStatusResponse, ForkSessionRequest, InitializeRequest, InitializeResponse, LoadSessionRequest,
    NewSessionRequest, NewSessionResponse, ResumeSessionRequest, SessionNotification, SessionOptionsRequest,
    SessionOptionsResponse,
};
use jsonrpc_nats::{RequestId, ResponseId};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
fn assert_fixture_preserved(expected: &Value, actual: &Value) {
    match expected {
        Value::Object(fields) => {
            let actual = actual.as_object().expect("expected an object");
            for (name, value) in fields {
                let actual = actual
                    .get(name)
                    .unwrap_or_else(|| panic!("missing field {name}: {actual:?}"));
                assert_fixture_preserved(value, actual);
            }
        }
        Value::Array(items) => {
            let actual = actual.as_array().expect("expected an array");
            assert_eq!(items.len(), actual.len());
            for (expected, actual) in items.iter().zip(actual) {
                assert_fixture_preserved(expected, actual);
            }
        }
        _ => assert_eq!(expected, actual),
    }
}

fn assert_request_round_trip<T: DeserializeOwned + Serialize>(method: &str, fixture: Value) -> Value {
    let incoming = encode_request(method, RequestId::Number(1), &fixture).unwrap();
    let decoded: T = decode_request_params(method, &incoming.headers, &incoming.body).unwrap();
    let outgoing = encode_request(method, RequestId::Number(1), &decoded).unwrap();
    let forwarded: Value = decode_request_params(method, &outgoing.headers, &outgoing.body).unwrap();
    assert_fixture_preserved(&fixture, &forwarded);
    forwarded
}

fn assert_notification_round_trip<T: DeserializeOwned + Serialize>(method: &str, fixture: Value) -> Value {
    let incoming = encode_notification(method, &fixture).unwrap();
    let decoded: T = decode_notification_params(method, &incoming.headers, &incoming.body).unwrap();
    let outgoing = encode_notification(method, &decoded).unwrap();
    let forwarded: Value = decode_notification_params(method, &outgoing.headers, &outgoing.body).unwrap();
    assert_fixture_preserved(&fixture, &forwarded);
    forwarded
}

fn assert_response_round_trip<T: DeserializeOwned + Serialize>(fixture: Value) -> Value {
    let incoming = encode_success(ResponseId::Number(1), &fixture).unwrap();
    let decoded: T = decode_response(&incoming.headers, &incoming.body).unwrap().unwrap();
    let outgoing = encode_success(ResponseId::Number(1), &decoded).unwrap();
    let forwarded: Value = decode_response(&outgoing.headers, &outgoing.body).unwrap().unwrap();
    assert_fixture_preserved(&fixture, &forwarded);
    forwarded
}

fn assert_session_update_round_trip(update: Value) -> Value {
    let fixture = json!({ "sessionId": "session-1", "update": update });
    assert_notification_round_trip::<SessionNotification>("session/update", fixture)["update"].clone()
}

fn initial_config_options() -> Value {
    json!([
        { "configId": "model", "value": "model-2", "_meta": { "source": "picker" } },
        { "configId": "fast", "type": "boolean", "value": true }
    ])
}

#[test]
fn session_options_request_and_response_survive_the_bridge() {
    assert_request_round_trip::<SessionOptionsRequest>(
        "session/options",
        json!({
            "cwd": "/workspace",
            "additionalDirectories": ["/workspace/shared"],
            "sessionId": "session-1",
            "_meta": { "probe": "options" }
        }),
    );
    assert_response_round_trip::<SessionOptionsResponse>(json!({
        "modes": {
            "currentModeId": "code",
            "availableModes": [{ "id": "code", "name": "Code" }]
        },
        "configOptions": [{
            "type": "select", "id": "speed", "name": "Speed", "category": "speed",
            "currentValue": "fast",
            "options": [{ "value": "fast", "name": "Fast" }, { "value": "careful", "name": "Careful" }]
        }],
        "_meta": { "snapshot": true }
    }));
}

#[test]
fn initial_config_options_survive_session_setup() {
    assert_request_round_trip::<NewSessionRequest>(
        "session/new",
        json!({ "cwd": "/workspace", "mcpServers": [], "configOptions": initial_config_options() }),
    );
    assert_request_round_trip::<LoadSessionRequest>(
        "session/load",
        json!({
            "sessionId": "session-1", "cwd": "/workspace", "mcpServers": [],
            "configOptions": initial_config_options()
        }),
    );
    assert_request_round_trip::<ResumeSessionRequest>(
        "session/resume",
        json!({ "sessionId": "session-1", "cwd": "/workspace", "configOptions": initial_config_options() }),
    );
}

#[test]
fn fork_cursor_and_initial_config_options_survive_the_bridge() {
    assert_request_round_trip::<ForkSessionRequest>(
        "session/fork",
        json!({
            "sessionId": "session-1", "cwd": "/workspace", "cursorId": "cur_1",
            "configOptions": initial_config_options()
        }),
    );
}

#[test]
fn speed_config_category_survives_the_bridge() {
    assert_response_round_trip::<NewSessionResponse>(json!({
        "sessionId": "session-1",
        "configOptions": [{
            "type": "select", "id": "speed", "name": "Speed", "category": "speed",
            "currentValue": "fast",
            "options": [{ "value": "fast", "name": "Fast" }]
        }]
    }));
}

#[test]
fn cursor_and_prompt_suggestion_capabilities_survive_the_bridge() {
    assert_request_round_trip::<InitializeRequest>(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "session": {
                    "cursors": {},
                    "promptSuggestions": { "image": {}, "audio": {}, "embeddedContext": {} }
                }
            }
        }),
    );
    assert_response_round_trip::<InitializeResponse>(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "sessionCapabilities": { "options": { "_meta": { "probe": "options" } } }
        }
    }));
}

// Upstream defines `SessionCursorsCapabilities` as an empty struct, so `_meta`
// on `clientCapabilities.session.cursors` cannot survive typed re-encode.
#[test]
fn cursor_capability_meta_is_dropped_by_the_schema() {
    let forwarded = assert_request_round_trip::<InitializeRequest>(
        "initialize",
        json!({ "protocolVersion": 1, "clientCapabilities": { "session": { "cursors": {} } } }),
    );
    let incoming = encode_request(
        "initialize",
        RequestId::Number(1),
        &json!({
            "protocolVersion": 1,
            "clientCapabilities": { "session": { "cursors": { "_meta": { "probe": "cursors" } } } }
        }),
    )
    .unwrap();
    let decoded: InitializeRequest = decode_request_params("initialize", &incoming.headers, &incoming.body).unwrap();
    let reencoded = serde_json::to_value(decoded).unwrap();
    assert_eq!(reencoded["clientCapabilities"]["session"]["cursors"], json!({}));
    assert_eq!(forwarded["clientCapabilities"]["session"]["cursors"], json!({}));
}

#[test]
fn cursor_updates_survive_the_bridge() {
    for anchor in [
        json!({ "type": "user_message", "messageId": "msg_user_2", "position": "before" }),
        json!({ "type": "agent_message", "messageId": "msg_agent_1", "position": "after" }),
        json!({ "type": "tool_call", "toolCallId": "call_1", "position": "after" }),
        json!({ "type": "compaction", "compactionId": "cmp_1", "position": "before" }),
    ] {
        assert_session_update_round_trip(json!({
            "sessionUpdate": "cursor_update",
            "cursorId": "cur_1",
            "capabilities": { "fork": {}, "_meta": { "probe": "caps" } },
            "anchor": anchor,
            "_meta": { "probe": "cursor" }
        }));
    }
}

#[test]
fn cursor_update_carrying_only_the_id_survives_the_bridge() {
    let forwarded = assert_session_update_round_trip(json!({ "sessionUpdate": "cursor_update", "cursorId": "cur_1" }));
    assert_eq!(
        forwarded,
        json!({ "sessionUpdate": "cursor_update", "cursorId": "cur_1" })
    );
}

#[test]
fn prompt_suggestions_updates_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "prompt_suggestions_update",
        "suggestions": [
            {
                "prompt": [{ "type": "text", "text": "Run the test suite and fix what breaks" }],
                "_meta": { "rank": 1 }
            },
            {
                "prompt": [
                    { "type": "text", "text": "Add tests for " },
                    { "type": "resource_link", "name": "parser.rs", "uri": "file:///workspace/src/parser.rs" }
                ]
            }
        ],
        "_meta": { "probe": "suggestions" }
    }));
}

#[test]
fn session_tool_call_content_survives_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call_1",
        "title": "Delegate",
        "content": [{ "type": "session", "sessionId": "sess_child_1", "_meta": { "probe": "child" } }]
    }));
}

#[test]
fn markdown_plan_path_survives_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "plan_update",
        "plan": {
            "type": "markdown",
            "planId": "plan-1",
            "content": "# Plan\n\n- Step 1",
            "path": "/workspace/PLAN.md"
        }
    }));
}

#[test]
fn auth_account_description_survives_the_bridge() {
    assert_response_round_trip::<AuthStatusResponse>(json!({
        "state": "authenticated",
        "accounts": [{
            "label": "work", "description": "Team workspace on the enterprise plan", "active": true
        }]
    }));
}
