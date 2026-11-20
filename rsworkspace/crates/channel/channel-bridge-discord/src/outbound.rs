use twilight_http::Client;
use twilight_http::response::Response;
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

/// Show the typing indicator in a channel. One trait per outbound operation;
/// never carries agent concepts.
#[allow(async_fn_in_trait)]
pub trait SendTyping {
    type Error: std::error::Error + 'static;
    type Output;

    async fn typing(&self, channel: Id<ChannelMarker>) -> Result<Self::Output, Self::Error>;
}

/// Send a text message to a channel. One trait per outbound operation; never
/// carries agent concepts.
#[allow(async_fn_in_trait)]
pub trait SendText {
    type Error: std::error::Error + 'static;
    type Message;

    async fn send_text(&self, channel: Id<ChannelMarker>, text: String) -> Result<Self::Message, Self::Error>;
}

pub struct DiscordOutbound {
    http: Client,
}

impl DiscordOutbound {
    pub fn new(http: Client) -> Self {
        Self { http }
    }
}

impl SendTyping for DiscordOutbound {
    type Error = twilight_http::Error;
    type Output = Response<twilight_http::response::marker::EmptyBody>;

    async fn typing(&self, channel: Id<ChannelMarker>) -> Result<Self::Output, Self::Error> {
        self.http.create_typing_trigger(channel).await
    }
}

impl SendText for DiscordOutbound {
    type Error = twilight_http::Error;
    type Message = Response<Message>;

    async fn send_text(&self, channel: Id<ChannelMarker>, text: String) -> Result<Self::Message, Self::Error> {
        self.http.create_message(channel).content(&text).await
    }
}
