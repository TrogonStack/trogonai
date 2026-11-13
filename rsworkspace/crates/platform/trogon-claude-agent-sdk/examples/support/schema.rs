use serde::Serialize;
use trogon_claude_agent_sdk::JsonObject;

#[derive(Serialize)]
struct EchoSchema {
    #[serde(rename = "type")]
    kind: &'static str,
    properties: EchoProperties,
    required: [&'static str; 1],
    #[serde(rename = "additionalProperties", skip_serializing_if = "Option::is_none")]
    additional_properties: Option<bool>,
}

#[derive(Serialize)]
struct EchoProperties {
    text: TextSchema,
}

#[derive(Serialize)]
struct TextSchema {
    #[serde(rename = "type")]
    kind: &'static str,
}

pub fn echo_schema(additional_properties: Option<bool>) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(serde_json::to_value(EchoSchema {
        kind: "object",
        properties: EchoProperties {
            text: TextSchema { kind: "string" },
        },
        required: ["text"],
        additional_properties,
    })?)
}
