#[cfg(test)]
mod tests;

use teloxide::types::{Update, UpdateKind};
use trogon_channel::{
    ChannelAccount, CommandTriggers, Endpoint, InboundEvent, MessageRef, PlatformUserId, SafeToken, Sender,
};

/// Normalize a raw Telegram update into the channel-neutral event, or `None` for
/// what the bridge does not carry: update kinds other than a new message (edits,
/// membership, ...) and messages with no words in them. Whatever is dropped here
/// stays on the raw stream with full fidelity for later.
pub fn inbound_event(update: &Update, account: &ChannelAccount, triggers: &CommandTriggers) -> Option<InboundEvent> {
    let UpdateKind::Message(msg) = &update.kind else {
        return None;
    };
    // Telegram files the words under `text` for a text message and under
    // `caption` for one that carries media, and teloxide keeps the two apart. A
    // caption is the user talking, and it costs no download to forward, so it
    // must not go missing while the media beside it waits for a downloader.
    let text = msg.text().or_else(|| msg.caption())?;
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
        // Empty until a downloader exists to redeem handles out of band, which is
        // ADR#0044's decision and not something this parser can anticipate: an
        // event that named an attachment nothing can resolve would promise bytes.
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
