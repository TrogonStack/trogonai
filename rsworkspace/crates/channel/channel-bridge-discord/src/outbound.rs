use crate::constants::TEXT_CHUNK_LIMIT;
use channel_acp_port::{SendText, SendTyping, chunk_utf16};
use twilight_http::Client;
use twilight_http::request::channel::message::CreateMessage;
use twilight_http::response::Response;
use twilight_model::channel::Message;
use twilight_model::channel::message::AllowedMentions;
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

#[cfg(test)]
mod tests;

/// Agent text is untrusted in a guild: a hallucinated or prompt-injected
/// `@everyone`, role or user mention would otherwise notify its targets. The
/// mentions still render; they just ping nobody.
static NO_PINGS: AllowedMentions = AllowedMentions {
    parse: Vec::new(),
    replied_user: false,
    roles: Vec::new(),
    users: Vec::new(),
};

pub struct DiscordOutbound {
    http: Client,
}

impl DiscordOutbound {
    pub fn new(http: Client) -> Self {
        Self { http }
    }
}

impl SendTyping<Id<ChannelMarker>> for DiscordOutbound {
    type Error = twilight_http::Error;
    type Output = Response<twilight_http::response::marker::EmptyBody>;

    async fn typing(&self, channel: Id<ChannelMarker>) -> Result<Self::Output, Self::Error> {
        self.http.create_typing_trigger(channel).await
    }
}

impl SendText<Id<ChannelMarker>> for DiscordOutbound {
    type Error = twilight_http::Error;
    type Message = Response<Message>;

    async fn send_text(&self, channel: Id<ChannelMarker>, text: &str) -> Result<Self::Message, Self::Error> {
        reply(&self.http, channel, text).await
    }

    fn chunks<'t>(&self, text: &'t str) -> Vec<&'t str> {
        chunk_utf16(text, TEXT_CHUNK_LIMIT)
    }
}

fn reply<'a>(http: &'a Client, channel: Id<ChannelMarker>, text: &'a str) -> CreateMessage<'a> {
    http.create_message(channel)
        .content(text)
        .allowed_mentions(Some(&NO_PINGS))
}
