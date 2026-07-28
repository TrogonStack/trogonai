use serde::{Deserialize, Serialize};

/// The channel-neutral output vocabulary: the one contract every channel
/// bridge implements. Kept deliberately small; platform-specific richness
/// (e.g. Telegram inline buttons) rides agent `_meta` and is rendered by the
/// bridge that understands it. This enum is the `chat.*.out.*` payload once
/// the multi-channel extraction happens.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RenderCommand {
    SendText {
        text: String,
    },
    /// Streaming preview: replace the text of a previously sent message.
    EditText {
        message_ref: String,
        text: String,
    },
    SendAttachment {
        object_ref: String,
        mime: String,
    },
    Typing,
    React {
        message_ref: String,
        emoji: String,
    },
}
