#[cfg(test)]
mod tests;

use channel_acp_port::DecodeInbound;
use teloxide::types::{Update, UpdateKind};
use tracing::warn;
use trogon_channel::{ChannelAccount, CommandTriggers, InboundEvent, MessageRef, PlatformUserId, SafeToken, Sender};

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

/// The bridge's [`DecodeInbound`]: a raw Telegram update in, the event and the
/// chat to answer in out.
pub struct TelegramDecoder<'a> {
    pub account: &'a ChannelAccount,
    pub triggers: &'a CommandTriggers,
}

impl DecodeInbound for TelegramDecoder<'_> {
    type ReplyTo = i64;

    fn decode(&self, body: &[u8]) -> Option<(InboundEvent, i64)> {
        let update = match serde_json::from_slice::<Update>(body) {
            Ok(update) => update,
            Err(e) => {
                warn!(error = %e, body_len = body.len(), "Unparseable Telegram update; dropping");
                return None;
            }
        };
        let event = inbound_event(&update, self.account, self.triggers)?;
        // `inbound_event` carries only new messages, and every message names
        // its chat.
        let chat_id = update.chat()?.id.0;
        Some((event, chat_id))
    }
}
