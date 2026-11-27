#[cfg(test)]
mod tests;

use channel_acp_port::DecodeInbound;
use tracing::warn;
use trogon_channel::{ChannelAccount, CommandTriggers, InboundEvent, MessageRef, PlatformUserId, SafeToken, Sender};
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, UserMarker};

/// A message the bridge carries, and where its reply goes.
///
/// The two differ for a direct message. Discord gives a DM its own channel id,
/// unrelated to the user's id, so keying the conversation on the channel would
/// mean no seeded user could ever be recognized in their own DMs. The
/// conversation is keyed on the user instead, the same endpoint a seeded
/// principal is linked to (and the same shape Telegram has, where a private
/// chat's id is the user's id), and the reply still goes to the DM channel.
#[derive(Debug)]
pub struct DiscordInbound {
    pub event: InboundEvent,
    pub reply_to: Id<ChannelMarker>,
}

/// Normalize a raw `MESSAGE_CREATE` into the channel-neutral event, or `None`
/// for what the bridge does not carry:
///
/// - messages from bots, this one included, so the bridge never answers itself
///   or another bot into a loop;
/// - guild messages that do not mention this bot, so a bot added to a server
///   speaks when spoken to rather than replying to every line in every channel;
/// - messages with no words left once the mention is removed.
///
/// Whatever is dropped here stays on the raw stream with full fidelity.
pub fn inbound(
    message: &Message,
    bot: Id<UserMarker>,
    account: &ChannelAccount,
    triggers: &CommandTriggers,
) -> Option<DiscordInbound> {
    if message.author.bot {
        return None;
    }
    let in_guild = message.guild_id.is_some();
    if in_guild && !message.mentions.iter().any(|mention| mention.id == bot) {
        return None;
    }

    let text = strip_mentions_of(&message.content, bot);
    if text.is_empty() {
        return None;
    }
    let parsed = triggers.parse(&text, account.account());

    let peer = if in_guild {
        SafeToken::from(message.channel_id.get())
    } else {
        SafeToken::from(message.author.id.get())
    };

    Some(DiscordInbound {
        event: InboundEvent {
            endpoint: account.endpoint_for(&peer),
            sender: Sender {
                platform_user_id: PlatformUserId::from(message.author.id.get()),
                display_name: message
                    .author
                    .global_name
                    .clone()
                    .unwrap_or_else(|| message.author.name.clone()),
            },
            text: parsed.body,
            command: parsed.command,
            // Empty until a downloader exists to redeem attachments out of band
            // (ADR#0044), exactly as for Telegram.
            attachments: Vec::new(),
            message_ref: MessageRef::new(message.id.get().to_string()).ok()?,
            occurred_at: message.timestamp.as_secs(),
        },
        reply_to: message.channel_id,
    })
}

/// Remove `<@id>` and `<@!id>` mentions of the bot: they address the message,
/// they are not part of what the user is saying.
fn strip_mentions_of(content: &str, bot: Id<UserMarker>) -> String {
    let id = bot.get();
    content
        .replace(&format!("<@{id}>"), "")
        .replace(&format!("<@!{id}>"), "")
        .trim()
        .to_string()
}

/// The bridge's [`DecodeInbound`]: a raw `MESSAGE_CREATE` in, the event and the
/// channel to answer in out.
pub struct DiscordDecoder<'a> {
    pub bot: Id<UserMarker>,
    pub account: &'a ChannelAccount,
    pub triggers: &'a CommandTriggers,
}

impl DecodeInbound for DiscordDecoder<'_> {
    type ReplyTo = Id<ChannelMarker>;

    fn decode(&self, body: &[u8]) -> Option<(InboundEvent, Id<ChannelMarker>)> {
        let message = match serde_json::from_slice::<Message>(body) {
            Ok(message) => message,
            Err(e) => {
                warn!(error = %e, body_len = body.len(), "Unparseable Discord message; dropping");
                return None;
            }
        };
        let DiscordInbound { event, reply_to } = inbound(&message, self.bot, self.account, self.triggers)?;
        Some((event, reply_to))
    }
}
