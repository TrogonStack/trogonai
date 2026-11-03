use rmcp::model::Tool;
use serde_json::{Map, json};

pub(crate) fn prompt_tool() -> Tool {
    let schema = Map::from_iter([
        ("type".into(), json!("object")),
        ("additionalProperties".into(), json!(false)),
        ("required".into(), json!(["session_id", "hook_event_name"])),
        (
            "properties".into(),
            json!({
                "session_id": {"type":"string","minLength":1,"maxLength":256},
                "prompt_id": {"type":"string","maxLength":256},
                "hook_event_name": {"type":"string","enum":["UserPromptSubmit"]}
            }),
        ),
    ]);
    Tool::new(
        "peer_prompt_hook",
        "Bind this connector to the native host session and prepare peer context for UserPromptSubmit.",
        schema,
    )
}
