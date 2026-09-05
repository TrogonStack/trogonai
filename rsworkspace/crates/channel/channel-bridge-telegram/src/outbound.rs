use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{ChatAction, ChatId, Message, True};

#[cfg(test)]
mod tests;

/// Show the typing indicator in a chat. One trait per outbound operation;
/// never carries agent concepts.
#[allow(async_fn_in_trait)]
pub trait SendTyping {
    type Error: std::error::Error + 'static;
    type Output;

    async fn typing(&self, chat_id: i64) -> Result<Self::Output, Self::Error>;
}

/// Send a text message to a chat. One trait per outbound operation; never
/// carries agent concepts.
#[allow(async_fn_in_trait)]
pub trait SendText {
    type Error: std::error::Error + 'static;
    type Message;

    async fn send_text(&self, chat_id: i64, text: String) -> Result<Self::Message, Self::Error>;
}

pub struct TelegramOutbound {
    bot: Bot,
}

impl TelegramOutbound {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }
}

impl SendTyping for TelegramOutbound {
    type Error = teloxide::RequestError;
    type Output = True;

    async fn typing(&self, chat_id: i64) -> Result<Self::Output, Self::Error> {
        self.bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await
    }
}

impl SendText for TelegramOutbound {
    type Error = teloxide::RequestError;
    type Message = Message;

    async fn send_text(&self, chat_id: i64, text: String) -> Result<Self::Message, Self::Error> {
        self.bot.send_message(ChatId(chat_id), text).await
    }
}
