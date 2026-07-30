use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId};

/// The render half's platform seam: what the pipeline needs from Telegram,
/// narrow enough to fake in tests. Grows with the render vocabulary
/// (edit-in-place, attachments), never with agent concepts.
#[allow(async_fn_in_trait)]
pub trait Outbound {
    async fn typing(&self, chat_id: i64) -> anyhow::Result<()>;
    async fn send_text(&self, chat_id: i64, text: String) -> anyhow::Result<()>;
}

pub struct TelegramOutbound {
    bot: Bot,
}

impl TelegramOutbound {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }
}

impl Outbound for TelegramOutbound {
    async fn typing(&self, chat_id: i64) -> anyhow::Result<()> {
        self.bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await?;
        Ok(())
    }

    async fn send_text(&self, chat_id: i64, text: String) -> anyhow::Result<()> {
        self.bot.send_message(ChatId(chat_id), text).await?;
        Ok(())
    }
}
