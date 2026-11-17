#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use acp_nats::wire::{
    decode_notification_params, decode_request_params, decode_response, encode_notification, encode_request,
    encode_success,
};
use agent_client_protocol::v1::schema::{
    ArchiveSessionResponse, AuthStatusResponse, AuthStatusUpdateNotification, CreateElicitationRequest,
    ForkSessionRequest, InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, MessageMcpNotification, MessageMcpRequest, MessageMcpResponse, NewSessionRequest,
    ResumeSessionRequest, SessionNotification, SessionStatusResponse, SetSessionTitleResponse,
    UnarchiveSessionResponse,
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

fn session_setup_fixture() -> Value {
    json!({
        "cwd": "/workspace",
        "additionalDirectories": ["/workspace/shared"],
        "mcpServers": [{ "type": "acp", "name": "tools", "serverId": "server-1" }],
        "systemPrompt": {
            "mode": "override",
            "content": [{ "type": "text", "text": "Answer concisely." }],
            "_meta": { "source": "client" }
        }
    })
}

fn existing_session_setup_fixture() -> Value {
    let mut fixture = session_setup_fixture();
    fixture["sessionId"] = json!("session-1");
    fixture
}

#[test]
fn tool_call_name_survives_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "tool-1",
        "title": "Read configuration",
        "name": "read_configuration"
    }));
}

#[test]
fn new_session_system_prompt_survives_the_bridge() {
    for mode in ["append", "override", "_provider_mode"] {
        let mut fixture = session_setup_fixture();
        fixture["systemPrompt"]["mode"] = json!(mode);
        assert_request_round_trip::<NewSessionRequest>("session/new", fixture);
    }
}

#[test]
fn load_session_system_prompt_survives_the_bridge() {
    assert_request_round_trip::<LoadSessionRequest>("session/load", existing_session_setup_fixture());
}

#[test]
fn fork_session_system_prompt_survives_the_bridge() {
    assert_request_round_trip::<ForkSessionRequest>("session/fork", existing_session_setup_fixture());
}

#[test]
fn resume_session_system_prompt_survives_the_bridge() {
    assert_request_round_trip::<ResumeSessionRequest>("session/resume", existing_session_setup_fixture());
}

#[test]
fn tool_call_update_name_survives_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "tool-1",
        "name": "read_configuration"
    }));
}

#[test]
fn client_compaction_notice_and_subagent_capabilities_survive_the_bridge() {
    assert_request_round_trip::<InitializeRequest>(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "session": { "compaction": {}, "notices": {} },
                "subagents": { "_meta": { "presentation": "tree" } },
                "auth": { "terminal": true }
            }
        }),
    );
}

#[test]
fn agent_lifecycle_and_auth_status_capabilities_survive_the_bridge() {
    assert_response_round_trip::<InitializeResponse>(json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "sessionCapabilities": {
                "archive": {}, "status": {}, "title": {}, "list": {},
                "systemPrompt": {
                    "mode": { "override": { "_meta": { "supported": true } } },
                    "_meta": { "delivery": "setup" }
                }
            },
            "auth": { "status": { "_meta": { "updates": "connection" } } }
        },
        "authMethods": [{
            "type": "terminal", "id": "login", "name": "Sign in",
            "description": "Open the terminal sign-in flow",
            "args": ["login"], "env": { "LOGIN_STYLE": "plain" },
            "_meta": { "presentation": "terminal" }
        }]
    }));
}

#[test]
fn session_notices_survive_the_bridge() {
    for severity in ["info", "warning", "error", "_provider_notice"] {
        assert_session_update_round_trip(json!({
            "sessionUpdate": "notice", "severity": severity,
            "title": "Context window nearly full",
            "description": "The next turn may compact earlier messages.",
            "_meta": { "source": "agent" }
        }));
    }
}

#[test]
fn compaction_updates_survive_the_bridge() {
    for status in ["in_progress", "completed", "failed", "cancelled", "_provider_state"] {
        assert_session_update_round_trip(json!({
            "sessionUpdate": "compaction_update", "compactionId": "compaction-1",
            "status": status,
            "summary": [{ "type": "text", "text": "Kept the configuration decision." }],
            "error": "A summary fragment could not be saved.",
            "_meta": { "attempt": 2 }
        }));
    }
}

#[test]
fn compaction_summary_chunks_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "compaction_summary_chunk", "compactionId": "compaction-1",
        "content": { "type": "text", "text": "Use the staging configuration." },
        "_meta": { "part": 1 }
    }));
}

#[test]
fn compaction_patch_clears_and_omissions_remain_distinct() {
    for patch in [json!({}), json!({ "summary": null, "error": null, "_meta": null })] {
        let mut fixture = json!({
            "sessionUpdate": "compaction_update", "compactionId": "compaction-1", "status": "in_progress"
        });
        fixture
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(assert_session_update_round_trip(fixture.clone()), fixture);
    }
}

#[test]
fn subagent_metadata_directories_and_capabilities_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "subagent_update", "sessionId": "child-1",
        "title": "Configuration reviewer", "description": "Review the workspace settings",
        "capabilities": { "cancel": { "_meta": { "reason": "client_control" } } },
        "state": { "state": "running", "_meta": { "phase": "review" } },
        "cwd": "/workspace/child", "additionalDirectories": ["/workspace/shared"],
        "_meta": { "display": "expanded" }
    }));
}

#[test]
fn subagent_idle_errors_and_usage_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "subagent_update", "sessionId": "child-1",
        "state": {
            "state": "idle", "stopReason": "error",
            "error": { "code": -32603, "message": "Review failed", "data": { "retryable": true } },
            "usage": {
                "totalTokens": 12, "inputTokens": 8, "outputTokens": 4,
                "thoughtTokens": 1, "cachedReadTokens": 2, "cachedWriteTokens": 3
            },
            "_meta": { "attempt": 1 }
        }
    }));
}

#[test]
fn subagent_future_state_and_stop_reason_fields_survive_the_bridge() {
    for state in [
        json!({ "state": "requires_action", "_meta": { "reason": "permission" } }),
        json!({ "state": "unknown", "_meta": { "reason": "reconnected" } }),
        json!({ "state": "_waiting", "queuePosition": 3 }),
        json!({ "state": "idle", "stopReason": "_handoff", "target": "child-2" }),
    ] {
        assert_session_update_round_trip(json!({
            "sessionUpdate": "subagent_update", "sessionId": "child-1", "state": state
        }));
    }
}

#[test]
fn subagent_patch_clears_and_omissions_remain_distinct() {
    for patch in [
        json!({}),
        json!({
            "title": null, "description": null, "capabilities": null, "state": null,
            "cwd": null, "additionalDirectories": null, "_meta": null
        }),
    ] {
        let mut fixture = json!({ "sessionUpdate": "subagent_update", "sessionId": "child-1" });
        fixture
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(assert_session_update_round_trip(fixture.clone()), fixture);
    }
}

#[test]
fn session_message_views_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "session_message", "messageId": "message-1",
        "senderSessionId": "session-1", "recipientSessionId": "child-1",
        "senderMessageId": "sent-1", "recipientMessageId": "received-1",
        "content": [{ "type": "text", "text": "Review this configuration." }],
        "_meta": { "correlation": "review-1" }
    }));
}

#[test]
fn session_message_patch_clears_and_omissions_remain_distinct() {
    for patch in [
        json!({}),
        json!({
            "senderSessionId": null, "recipientSessionId": null,
            "senderMessageId": null, "recipientMessageId": null, "content": null, "_meta": null
        }),
    ] {
        let mut fixture = json!({ "sessionUpdate": "session_message", "messageId": "message-1" });
        fixture
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(assert_session_update_round_trip(fixture.clone()), fixture);
    }
}

#[test]
fn session_message_chunks_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "session_message_chunk", "messageId": "message-1",
        "content": { "type": "text", "text": "The configuration is valid." },
        "_meta": { "part": 1 }
    }));
}

#[test]
fn session_archive_filters_survive_the_bridge() {
    for archived in ["exclude", "include", "only", "_provider_filter"] {
        assert_request_round_trip::<ListSessionsRequest>(
            "session/list",
            json!({
                "cwd": "/workspace", "cursor": "next-page", "archived": archived
            }),
        );
    }
}

#[test]
fn listed_session_archive_metadata_survives_the_bridge() {
    assert_response_round_trip::<ListSessionsResponse>(json!({
        "sessions": [
            { "sessionId": "saved-1", "cwd": "/workspace", "additionalDirectories": ["/workspace/shared"],
              "title": "Archived review", "updatedAt": "2026-11-01T12:00:00Z", "archived": true,
              "_meta": { "owner": "client" } },
            { "sessionId": "saved-2", "cwd": "/workspace", "archived": false }
        ],
        "nextCursor": "next-page", "_meta": { "page": 1 }
    }));
}

#[test]
fn usage_costs_in_non_iso_units_survive_the_bridge() {
    assert_session_update_round_trip(json!({
        "sessionUpdate": "usage_update", "used": 10, "size": 100,
        "cost": { "amount": 1.25, "currency": "credits", "_meta": { "unit": "provider" } }
    }));
}

#[test]
fn request_scoped_mcp_requests_survive_the_bridge() {
    assert_request_round_trip::<MessageMcpRequest>(
        "mcp/message",
        json!({
            "serverId": "server-1", "requestId": "inner-1", "method": "tools/call",
            "params": { "name": "read_configuration", "arguments": { "format": "json" } },
            "_meta": { "route": "client" }
        }),
    );
}

#[test]
fn request_scoped_mcp_notifications_survive_the_bridge() {
    assert_notification_round_trip::<MessageMcpNotification>(
        "mcp/message",
        json!({
            "serverId": "server-1", "requestId": "inner-1", "method": "notifications/progress",
            "params": { "progressToken": "read-1", "progress": 5, "total": 10 },
            "_meta": { "route": "agent" }
        }),
    );
}

#[test]
fn mcp_result_extensions_and_explicit_null_survive_the_bridge() {
    for result in [
        json!(null),
        json!({
            "content": [{ "type": "text", "text": "configuration" }],
            "structuredContent": { "enabled": true }, "_provider": { "version": 2 }
        }),
    ] {
        assert_response_round_trip::<MessageMcpResponse>(json!({
            "result": result, "_meta": { "server": "tools" }
        }));
    }
}

#[test]
fn mcp_error_extensions_and_data_null_survive_the_bridge() {
    for data in [json!(null), json!({ "retryable": true })] {
        assert_response_round_trip::<MessageMcpResponse>(json!({
            "error": { "code": -32602, "message": "Invalid tool argument", "data": data,
                       "_provider": { "classification": "validation" } },
            "_meta": { "server": "tools" }
        }));
    }
}

#[test]
fn mcp_error_data_omission_remains_distinct_from_null() {
    for error in [
        json!({ "code": -32602, "message": "Invalid tool argument" }),
        json!({ "code": -32602, "message": "Invalid tool argument", "data": null }),
    ] {
        let fixture = json!({ "error": error });
        assert_eq!(
            assert_response_round_trip::<MessageMcpResponse>(fixture.clone()),
            fixture
        );
    }
}

fn auth_status_fixture() -> Value {
    json!({
        "state": "authenticated", "accounts": [{
            "label": "Example account", "active": true, "name": "Test user",
            "email": concat!("test-account", "@", "example.invalid"), "organization": "Example team",
            "_meta": { "provider": "example" }
        }],
        "_meta": { "source": "agent" }
    })
}

#[test]
fn auth_status_accounts_survive_the_bridge() {
    assert_response_round_trip::<AuthStatusResponse>(auth_status_fixture());
}

#[test]
fn auth_status_update_payloads_survive_the_wire_codec() {
    assert_notification_round_trip::<AuthStatusUpdateNotification>("auth/status_update", auth_status_fixture());
}

#[test]
fn session_status_and_title_responses_survive_the_bridge() {
    for status in ["open", "saved", "_provider_status"] {
        assert_response_round_trip::<SessionStatusResponse>(json!({
            "status": status, "_meta": { "source": "agent" }
        }));
    }
    assert_response_round_trip::<SetSessionTitleResponse>(json!({
        "title": "Updated review title", "_meta": { "source": "client" }
    }));
}

#[test]
fn archive_response_metadata_survives_the_bridge() {
    let fixture = json!({ "_meta": { "source": "agent", "revision": 2 } });
    assert_response_round_trip::<ArchiveSessionResponse>(fixture.clone());
    assert_response_round_trip::<UnarchiveSessionResponse>(fixture);
}

#[test]
fn stabilized_elicitation_form_and_custom_modes_survive_the_bridge() {
    assert_request_round_trip::<CreateElicitationRequest>(
        "elicitation/create",
        json!({
            "mode": "form", "sessionId": "session-1", "toolCallId": "tool-1",
            "message": "Select a configuration", "requestedSchema": {
                "type": "object", "properties": { "choice": {
                    "type": "string", "oneOf": [{
                        "const": "staging", "title": "Staging", "description": "Use the staging environment"
                    }]
                } }, "required": ["choice"]
            }
        }),
    );
    assert_request_round_trip::<CreateElicitationRequest>(
        "elicitation/create",
        json!({
            "mode": "_provider_choice", "requestId": "auth-1", "message": "Choose an account",
            "choices": [{ "id": "example" }], "_meta": { "presentation": "selector" }
        }),
    );
}
