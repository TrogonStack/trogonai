use rmcp::model::Tool;

use crate::model_inputs::{AcknowledgeInput, RosterInput, SendInput, StatusInput};

pub(crate) fn model_tool(name: &str) -> Option<Tool> {
    match name {
        "peer_roster" => Some(tool::<RosterInput>(
            "peer_roster",
            "List the peers you can message. Each entry has a displayName and the 64-hex peerId to pass as \
             recipient_peer_id. Call it with no arguments. Entries are presence observations and do not grant \
             access. New peer messages are delivered to you automatically; do not poll for them.",
        )),
        "peer_send" => Some(tool::<SendInput>(
            "peer_send",
            "Send a text message to another agent (a peer). recipient_peer_id is the exact 64-hex peerId from \
             peer_roster or from a delivered message's sender_peer_id; never a display name and never your own \
             peerId. Choose a new idempotency_key for each new message, e.g. \"ask-api-1\"; reuse a key only to \
             retry that same message with the same text. Your chat replies are never seen by peers; only \
             peer_send reaches them. Replies are delivered to you automatically.",
        )),
        "peer_status" => Some(tool::<StatusInput>(
            "peer_status",
            "Delivery status of a message you sent, looked up by the idempotency_key you passed to peer_send. It \
             never returns replies or message text; replies from peers are delivered to you automatically.",
        )),
        "peer_acknowledge_batch" => Some(tool::<AcknowledgeInput>(
            "peer_acknowledge_batch",
            "After reading delivered peer messages, acknowledge them once with that delivery's batch_id. A batch_id \
             is not a message_id or an idempotency key; never guess one. Newer peer messages wait until you \
             acknowledge. This records application acknowledgment only, not host insertion, understanding, or \
             task completion.",
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
