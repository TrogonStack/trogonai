use rmcp::model::Tool;

use crate::model_inputs::{AcknowledgeInput, RosterInput, SendInput, StatusInput};

pub(crate) fn model_tool(name: &str) -> Option<Tool> {
    match name {
        "peer_roster" => Some(tool::<RosterInput>(
            "peer_roster",
            "Read authorized peer presence metadata; roster observations do not grant access.",
        )),
        "peer_send" => Some(tool::<SendInput>(
            "peer_send",
            "Send bounded text to an exact peer using a stable idempotency key; retries must reuse the same key and content.",
        )),
        "peer_status" => Some(tool::<StatusInput>(
            "peer_status",
            "Read the receipt/status for an original sender-scoped idempotency key; no message bodies are returned.",
        )),
        "peer_acknowledge_batch" => Some(tool::<AcknowledgeInput>(
            "peer_acknowledge_batch",
            "Record application acknowledgment for an assigned batch; this does not prove host insertion, understanding, or task completion.",
        )),
        _ => None,
    }
}

fn tool<T: schemars::JsonSchema + 'static>(name: &'static str, description: &'static str) -> Tool {
    let mut tool = Tool::default().with_input_schema::<T>();
    tool.name = name.into();
    tool.description = Some(description.into());
    tool
}
