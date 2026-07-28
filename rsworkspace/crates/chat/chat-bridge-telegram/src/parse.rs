use teloxide::types::{Update, UpdateKind};
use trogon_chat::{Endpoint, InboundChatEvent, Sender};

/// Normalize a raw Telegram update into the channel-neutral event, or `None`
/// for update kinds v1 does not carry (media, edits, membership, ...). The
/// raw stream retains those with full fidelity for later.
pub fn inbound_event(update: &Update, bot_account: &str) -> Option<InboundChatEvent> {
    let UpdateKind::Message(msg) = &update.kind else {
        return None;
    };
    let text = msg.text()?;
    let from = msg.from.as_ref()?;

    let endpoint = match Endpoint::new("telegram", bot_account, msg.chat.id.0.to_string()) {
        Ok(endpoint) => endpoint,
        Err(e) => {
            tracing::warn!(error = %e, chat_id = msg.chat.id.0, "Skipping update with unencodable endpoint");
            return None;
        }
    };

    Some(InboundChatEvent {
        endpoint,
        sender: Sender {
            platform_user_id: from.id.0.to_string(),
            display_name: from.full_name(),
        },
        text: Some(text.to_string()),
        attachments: Vec::new(),
        message_ref: msg.id.0.to_string(),
        occurred_at: msg.date.timestamp(),
    })
}
