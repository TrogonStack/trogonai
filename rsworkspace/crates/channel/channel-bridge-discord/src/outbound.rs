use crate::constants::TEXT_CHUNK_LIMIT;
use crate::render::chunk_text;
use channel_acp_port::{SendText, SendTyping};
use twilight_http::Client;
use twilight_http::response::Response;
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

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
        self.http.create_message(channel).content(text).await
    }

    fn chunks<'t>(&self, text: &'t str) -> Vec<&'t str> {
        chunk_text(text, TEXT_CHUNK_LIMIT)
    }
}
