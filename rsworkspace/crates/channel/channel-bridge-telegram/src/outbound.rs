use crate::constants::TEXT_CHUNK_LIMIT;
use channel_acp_port::{SendText, SendTyping, chunk_utf16};
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId, Message, True};

#[cfg(test)]
mod tests;

pub struct TelegramOutbound {
    bot: Bot,
}

impl TelegramOutbound {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }
}

impl SendTyping<i64> for TelegramOutbound {
    type Error = teloxide::RequestError;
    type Output = True;

    async fn typing(&self, chat_id: i64) -> Result<Self::Output, Self::Error> {
        self.bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await
    }
}

impl SendText<i64> for TelegramOutbound {
    type Error = teloxide::RequestError;
    type Message = Message;

    async fn send_text(&self, chat_id: i64, text: &str) -> Result<Self::Message, Self::Error> {
        self.bot.send_message(ChatId(chat_id), text).await
    }

    fn chunks<'t>(&self, text: &'t str) -> Vec<&'t str> {
        chunk_utf16(text, TEXT_CHUNK_LIMIT)
    }
}
