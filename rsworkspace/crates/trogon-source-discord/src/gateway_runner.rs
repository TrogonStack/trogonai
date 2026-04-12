use std::future::poll_fn;
use std::pin::Pin;

use futures_core::Stream;
use tracing::{info, warn};
use twilight_gateway::{Message, Shard, ShardId};

use crate::gateway::GatewayBridge;

pub async fn run<
    P: trogon_nats::jetstream::JetStreamPublisher,
    S: trogon_nats::jetstream::ObjectStorePut,
>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: &crate::config::DiscordConfig,
) {
    info!("mode: gateway");

    let bridge = GatewayBridge::new(
        publisher,
        config.subject_prefix.clone(),
        config.nats_ack_timeout.into(),
    );

    let mut shard = Shard::new(
        ShardId::ONE,
        config.bot_token.as_str().to_owned(),
        config.intents,
    );

    info!("starting Discord gateway connection");

    loop {
        let msg = poll_fn(|cx| Stream::poll_next(Pin::new(&mut shard), cx)).await;
        match msg {
            Some(Ok(Message::Text(text))) => bridge.dispatch(&text).await,
            Some(Ok(Message::Close(_))) => {
                info!("gateway connection closed");
                break;
            }
            Some(Err(source)) => {
                warn!(?source, "error receiving gateway message");
                continue;
            }
            None => break,
        }
    }
}
