#[cfg(test)]
mod tests;

use teloxide::types::{Update, UpdateKind};
use trogon_channel::{
    ChannelAccount, CommandTriggers, Endpoint, InboundEvent, MessageRef, PlatformUserId, SafeToken, Sender,
};

/// Normalize a raw Telegram update into the channel-neutral event, or `None`
/// for update kinds the bridge does not carry (media, edits, membership, ...).
/// The raw stream retains those with full fidelity for later.
pub fn inbound_event(update: &Update, account: &ChannelAccount, triggers: &CommandTriggers) -> Option<InboundEvent> {
    let UpdateKind::Message(msg) = &update.kind else {
        return None;
    };
    let text = msg.text()?;
    let from = msg.from.as_ref()?;

    let parsed = triggers.parse(text, account.account());

    // Telegram numbers chats, users, and messages, and every value object below
    // takes an integer without a failure case, so nothing an update carries can
    // spoil this event. The account was checked once, at boot.
    Some(InboundEvent {
        endpoint: account.endpoint_for(&SafeToken::from(msg.chat.id.0)),
        sender: Sender {
            platform_user_id: PlatformUserId::from(from.id.0),
            display_name: from.full_name(),
        },
        text: parsed.body,
        command: parsed.command,
        attachments: Vec::new(),
        message_ref: MessageRef::from(i64::from(msg.id.0)),
        occurred_at: msg.date.timestamp(),
    })
}

/// The endpoint of whoever sent a message, which is not the conversation's
/// endpoint: a group chat is one endpoint shared by everyone in it, so
/// authorizing the chat says nothing about authorizing the speaker. Named rather
/// than inlined because that distinction is the only thing it exists to keep.
pub fn sender_endpoint(account: &ChannelAccount, sender: &Sender) -> Endpoint {
    account.endpoint_for(sender.platform_user_id.token())
}
