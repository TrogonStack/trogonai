use crate::command::Command;
use crate::endpoint::Endpoint;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sender {
    pub platform_user_id: String,
    pub display_name: String,
}

/// Media that arrived with a message, already claim-checked: the bytes live in
/// the object store and `object_ref` points at them. `platform_ref` keeps the
/// platform's own handle (e.g. a Telegram `file_id`) for provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: String,
    pub mime: String,
    pub size: u64,
    pub object_ref: String,
    pub platform_ref: String,
}

/// A normalized inbound message: what any channel bridge produces after
/// stripping its platform's shape. This type is the `chat.*.in.*` payload
/// once the multi-channel extraction happens; until then it travels
/// in-process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEvent {
    pub endpoint: Endpoint,
    pub sender: Sender,
    /// Message text with any command trigger already removed, so what reaches
    /// the agent is only what the user meant for it.
    pub text: Option<String>,
    /// A bridge command found in the text. Extracted at the channel edge
    /// because the trigger vocabulary is a channel affordance; acted on by the
    /// routing layer and never forwarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Platform message identity, for dedup, replies, and edits.
    pub message_ref: String,
    /// Unix seconds, as reported by the platform.
    pub occurred_at: i64,
}
