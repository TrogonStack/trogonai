use super::*;
use serde_json::json;

#[test]
fn serializes_an_agent_surface_update_with_catalog_properties() {
    let message = AgentToRendererMessage {
        version: ProtocolVersion::V1_0,
        message: AgentToRendererPayload::UpdateComponents {
            update_components: UpdateComponents {
                surface_id: "main".to_owned(),
                components: vec![
                    Component::new("root", "Card").with_property("child", json!("title")),
                    Component::new("title", "Text").with_property("text", json!("Hello")),
                ],
            },
        },
    };

    let value = serde_json::to_value(message).expect("message should serialize");
    assert_eq!(value["version"], "1.0");
    assert_eq!(value["updateComponents"]["components"][1]["text"], "Hello");
}

#[test]
fn round_trips_a_renderer_action() {
    let value = json!({
        "version": "1.0",
        "action": {
            "name": "confirm",
            "surfaceId": "booking",
            "sourceComponentId": "submit",
            "timestamp": "2026-11-23T12:00:00Z",
            "context": {"seat": "12A"}
        }
    });

    let message: RendererToAgentMessage = serde_json::from_value(value.clone()).expect("message should parse");
    assert_eq!(serde_json::to_value(message).expect("message should serialize"), value);
}

#[test]
fn preserves_optional_data_model_path() {
    let value = json!({
        "version": "1.0",
        "updateDataModel": {
            "surfaceId": "main",
            "value": {"ready": true}
        }
    });

    let message: AgentToRendererMessage = serde_json::from_value(value.clone()).expect("message should parse");
    assert_eq!(serde_json::to_value(message).expect("message should serialize"), value);
}

#[test]
fn serializes_renderer_function_calls_with_required_catalog_id() {
    let message = AgentToRendererMessage {
        version: ProtocolVersion::V1_0,
        message: AgentToRendererPayload::CallRendererFunction {
            call_renderer_function: CallRendererFunction {
                surface_id: None,
                function_call_id: "call-1".to_owned(),
                call_function: RendererFunctionCall {
                    call: "openUrl".to_owned(),
                    catalog_id: "basic".to_owned(),
                    args: Some(Map::from_iter([("url".to_owned(), json!("https://example.com"))])),
                },
            },
        },
    };

    let value = serde_json::to_value(message).expect("message should serialize");
    assert_eq!(value["callRendererFunction"]["functionCallId"], "call-1");
    assert_eq!(value["callRendererFunction"]["callFunction"]["catalogId"], "basic");
    assert!(value["callRendererFunction"].get("surfaceId").is_none());
}

#[test]
fn rejects_unknown_protocol_versions() {
    let value = json!({
        "version": "2.0",
        "action": {
            "name": "confirm",
            "surfaceId": "booking",
            "sourceComponentId": "submit",
            "timestamp": "2026-11-23T12:00:00Z",
            "context": {}
        }
    });

    assert!(serde_json::from_value::<RendererToAgentMessage>(value).is_err());
}
