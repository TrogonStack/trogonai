use rmcp::model::Tool;
use serde_json::{Map, Value, json};

pub(crate) fn model_tool(name: &str) -> Option<Tool> {
    match name {
        "peer_roster" => Some(tool(
            "peer_roster",
            "Read authorized peer presence metadata; roster observations do not grant access.",
            json!({"page_size":{"type":"integer","minimum":0,"maximum":4294967295_u64},"page_token":{"type":"string"}}),
            &[],
        )),
        "peer_send" => Some(tool(
            "peer_send",
            "Send bounded text to an exact peer using a stable idempotency key; retries must reuse the same key and content.",
            json!({
                "recipient_peer_id":{"type":"string","pattern":"^[0-9a-f]{64}$"},
                "idempotency_key":token_schema(),
                "text":{"type":"string","minLength":1,"maxLength":2048,"description":"At most 2048 UTF-8 bytes."},
                "reply_to_message_id":token_schema()
            }),
            &["recipient_peer_id", "idempotency_key", "text"],
        )),
        "peer_status" => Some(tool(
            "peer_status",
            "Read the receipt/status for an original sender-scoped idempotency key; no message bodies are returned.",
            json!({"idempotency_key":token_schema()}),
            &["idempotency_key"],
        )),
        "peer_acknowledge_batch" => Some(tool(
            "peer_acknowledge_batch",
            "Record application acknowledgment for an assigned batch; this does not prove host insertion, understanding, or task completion.",
            json!({"batch_id":token_schema()}),
            &["batch_id"],
        )),
        _ => None,
    }
}

fn token_schema() -> Value {
    json!({"type":"string","pattern":"^[A-Za-z0-9_-]{1,128}$"})
}

fn tool(name: &'static str, description: &'static str, properties: Value, required: &[&str]) -> Tool {
    let schema = Map::from_iter([
        ("type".into(), json!("object")),
        ("additionalProperties".into(), json!(false)),
        ("properties".into(), properties),
        ("required".into(), json!(required)),
    ]);
    Tool::new(name, description, schema)
}
