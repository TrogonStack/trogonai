use teloxide::types::{Update, UpdateKind};
use trogon_channel::{CommandTriggers, Endpoint, InboundEvent, Sender};

/// Normalize a raw Telegram update into the channel-neutral event, or `None`
/// for update kinds the bridge does not carry (media, edits, membership, ...).
/// The raw stream retains those with full fidelity for later.
pub fn inbound_event(update: &Update, bot_account: &str, triggers: &CommandTriggers) -> Option<InboundEvent> {
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

    let parsed = triggers.parse(text);

    Some(InboundEvent {
        endpoint,
        sender: Sender {
            platform_user_id: from.id.0.to_string(),
            display_name: from.full_name(),
        },
        text: parsed.body,
        command: parsed.command,
        attachments: Vec::new(),
        message_ref: msg.id.0.to_string(),
        occurred_at: msg.date.timestamp(),
    })
}

/// The endpoint of whoever sent a message, which is not the conversation's
/// endpoint: a group chat is one endpoint shared by everyone in it, so
/// authorizing the chat says nothing about authorizing the speaker.
pub fn sender_endpoint(bot_account: &str, sender: &Sender) -> Option<Endpoint> {
    match Endpoint::new("telegram", bot_account, sender.platform_user_id.clone()) {
        Ok(endpoint) => Some(endpoint),
        Err(e) => {
            tracing::warn!(error = %e, user_id = %sender.platform_user_id, "Sender has an unencodable endpoint");
            None
        }
    }
}
